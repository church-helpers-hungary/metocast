use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;
use std::path::Path;

use crate::server::AppState;

#[path = "presenter_svg.rs"]
pub(crate) mod presenter_svg;

/// About five lines at 38 pt: more than this and the reading is too dense to follow from the pews.
const MAX_BIBLE_WORDS_PER_SLIDE: usize = 35;
const BIBLE_MAIN_FONT_SIZE_PT: f32 = 38.0;
const BIBLE_COUNTER_FONT_SIZE_PT: f32 = 18.0;
const SONG_FONT_SIZE_PT: f32 = 48.0;
const SONG_TITLE_FONT_SIZE_PT: f32 = 60.0;
/// The longest line that keeps 48 pt inside a generated deck's margins.
const SONG_LINE_CHARS: usize = 34;
/// Past this, a line is a paragraph: it stays whole and wraps inside the slide.
const SONG_PARAGRAPH_CHARS: usize = 150;
/// Counts bullets, not the lines a long bullet wraps onto.
const ANNOUNCEMENTS_PER_SLIDE: usize = 5;

// ── Data types ────────────────────────────────────────────────────────────────

/// A single paragraph from a slide, with each visual line stored separately.
///
/// `lines` contains the text of each visual line; `<a:br>` (Shift+Enter) in
/// the PPTX produces one entry per break.  `align` is a CSS keyword.
/// `font_size_pt` is the author-specified size in points (0.0 = not found).
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphContent {
    pub lines: Vec<String>,
    pub align: String,
    pub font_size_pt: f32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SlideContent {
    pub index: u32,
    pub paragraphs: Vec<ParagraphContent>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SvgSlideContent {
    pub index: u32,
    pub svg: String,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum PresenterRenderMode {
    #[default]
    Text,
    Svg,
}

use metocast_core::protocol::MAX_WS_MESSAGE_BYTES;
pub use metocast_core::protocol::PresenterTheme;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BibleReferenceType {
    Textus,
    Leckio,
}

impl BibleReferenceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Textus => "textus",
            Self::Leckio => "leckio",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Textus => "Textus",
            Self::Leckio => "Lekció",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibleVerseContent {
    pub chapter: u32,
    pub verse: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
struct BibleVerseChunk {
    chapter: u32,
    verse: u32,
    text: String,
}

#[derive(Debug, Clone)]
struct BibleSlidePage {
    chunks: Vec<BibleVerseChunk>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPresentation {
    pub file_path: String,
    pub total_slides: u32,
    pub slides: Vec<SlideContent>,
    /// Slide width in EMUs (English Metric Units; 914 400 EMU = 1 inch).
    pub slide_width_emu: u64,
    /// Slide height in EMUs.
    pub slide_height_emu: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParsedSvgPresentation {
    pub file_path: String,
    pub total_slides: u32,
    pub slides: Vec<SlideContent>,
    pub svg_slides: Vec<SvgSlideContent>,
    pub slide_width_emu: u64,
    pub slide_height_emu: u64,
}

// ── PPTX parsing ──────────────────────────────────────────────────────────────

fn slide_number(basename: &str) -> Option<u32> {
    basename
        .strip_suffix(".xml")
        .and_then(|s| s.strip_prefix("slide"))
        .and_then(|n| n.parse::<u32>().ok())
}

fn map_align(raw: &[u8]) -> &'static str {
    match raw {
        b"ctr" => "center",
        b"r" => "right",
        b"just" | b"dist" => "justify",
        _ => "left",
    }
}

/// Read the `sz` attribute (centipoints) from a run-properties element and
/// convert to points.  Returns 0.0 if the attribute is absent or unparseable.
fn read_sz(e: &quick_xml::events::BytesStart<'_>) -> f32 {
    for attr in e.attributes().flatten() {
        if attr.key.into_inner() == b"sz"
            && let Ok(s) = std::str::from_utf8(attr.value.as_ref())
            && let Ok(n) = s.parse::<u32>()
        {
            return n as f32 / 100.0;
        }
    }
    0.0
}

fn is_decorative_shape(e: &quick_xml::events::BytesStart<'_>) -> bool {
    e.attributes().flatten().any(|attr| {
        attr.key.into_inner() == b"name" && attr.value.as_ref().starts_with(b"Metocast Decorative")
    })
}

/// Trim `current` and push it to `lines` if non-empty; then clear `current`.
fn push_line(lines: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim().to_string();
    current.clear();
    if !trimmed.is_empty() {
        lines.push(trimmed);
    }
}

/// Parse all visible text from a single slide's XML.
///
/// Each `<a:p>` becomes a `ParagraphContent`.  `<a:br>` (Shift+Enter in PPT)
/// is honoured as an explicit visual line break within the paragraph — these
/// are always intentional author decisions, never automatic word-wrap artefacts.
/// The paragraph font size is taken from the first run/default-run properties
/// element (`<a:rPr>`, `<a:defRPr>`, `<a:endParaRPr>`) that carries an `sz`
/// attribute.
fn parse_slide_xml(xml: &[u8]) -> Vec<ParagraphContent> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_reader(xml);
    let mut buf: Vec<u8> = Vec::new();
    let mut paragraphs: Vec<ParagraphContent> = Vec::new();

    let mut para_lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut current_align = "left";
    let mut font_size_pt: f32 = 0.0;
    let mut in_text_run = false;
    let mut ignore_shape = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().into_inner() {
                b"p:sp" => ignore_shape = false,
                b"p:cNvPr" => ignore_shape = is_decorative_shape(e),
                b"a:p" => {
                    if !ignore_shape {
                        para_lines.clear();
                        current_line.clear();
                        current_align = "left";
                        font_size_pt = 0.0;
                    }
                }
                b"a:pPr" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.into_inner() == b"algn" {
                            current_align = map_align(attr.value.as_ref());
                            break;
                        }
                    }
                }
                b"a:rPr" | b"a:defRPr" | b"a:endParaRPr" => {
                    if font_size_pt == 0.0 {
                        font_size_pt = read_sz(e);
                    }
                }
                b"a:br" => {
                    push_line(&mut para_lines, &mut current_line);
                }
                b"a:t" => in_text_run = !ignore_shape,
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.name().into_inner() {
                b"p:cNvPr" => ignore_shape = is_decorative_shape(e),
                b"a:pPr" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.into_inner() == b"algn" {
                            current_align = map_align(attr.value.as_ref());
                            break;
                        }
                    }
                }
                b"a:rPr" | b"a:defRPr" | b"a:endParaRPr" => {
                    if font_size_pt == 0.0 {
                        font_size_pt = read_sz(e);
                    }
                }
                b"a:br" => {
                    push_line(&mut para_lines, &mut current_line);
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) => {
                if in_text_run && let Ok(decoded) = e.decode() {
                    let unescaped = quick_xml::escape::unescape(&decoded)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| decoded.into_owned());
                    current_line.push_str(&unescaped);
                }
            }
            Ok(Event::End(ref e)) => match e.name().into_inner() {
                b"a:t" => in_text_run = false,
                b"a:p" if !ignore_shape => {
                    push_line(&mut para_lines, &mut current_line);
                    if !para_lines.is_empty() {
                        paragraphs.push(ParagraphContent {
                            lines: std::mem::take(&mut para_lines),
                            align: current_align.to_string(),
                            font_size_pt,
                        });
                    }
                }
                b"p:sp" => ignore_shape = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    paragraphs
}

/// Parse `ppt/presentation.xml` for the `<p:sldSz>` element and return its
/// `cx`/`cy` attributes in EMUs.  Falls back to the standard 16:9 dimensions.
fn parse_slide_size(xml: &[u8]) -> (u64, u64) {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_reader(xml);
    let mut buf: Vec<u8> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                if e.name().into_inner() == b"p:sldSz" {
                    let mut cx = 0u64;
                    let mut cy = 0u64;
                    for attr in e.attributes().flatten() {
                        match attr.key.into_inner() {
                            b"cx" => {
                                if let Ok(s) = std::str::from_utf8(attr.value.as_ref()) {
                                    cx = s.parse().unwrap_or(0);
                                }
                            }
                            b"cy" => {
                                if let Ok(s) = std::str::from_utf8(attr.value.as_ref()) {
                                    cy = s.parse().unwrap_or(0);
                                }
                            }
                            _ => {}
                        }
                    }
                    if cx > 0 && cy > 0 {
                        return (cx, cy);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    (12192000, 6858000) // default 16:9 widescreen
}

/// Parse a `.pptx` file and return structured slide content.
pub fn parse_pptx(file_path: &str) -> Result<ParsedPresentation, String> {
    let file = std::fs::File::open(file_path).map_err(|e| format!("Cannot open file: {e}"))?;

    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid .pptx file: {e}"))?;

    // Collect all slide entry names from "ppt/slides/slideN.xml".
    let mut slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let name = archive.by_index(i).ok()?.name().to_string();
            let basename = name.strip_prefix("ppt/slides/")?;
            if basename.contains('/') {
                return None;
            }
            slide_number(basename)?;
            Some(name)
        })
        .collect();

    if slide_names.is_empty() {
        return Err(
            "No slides found. Only .pptx format is supported — please re-save .ppt files as .pptx."
                .to_string(),
        );
    }

    slide_names.sort_by_key(|name| {
        name.strip_prefix("ppt/slides/")
            .and_then(slide_number)
            .unwrap_or(0)
    });

    let total = slide_names.len() as u32;
    let mut slides: Vec<SlideContent> = Vec::with_capacity(slide_names.len());

    for (idx, name) in slide_names.iter().enumerate() {
        let mut xml_bytes: Vec<u8> = Vec::new();
        archive
            .by_name(name)
            .map_err(|e| format!("Cannot open slide entry '{name}': {e}"))?
            .read_to_end(&mut xml_bytes)
            .map_err(|e| format!("Cannot read slide content: {e}"))?;

        slides.push(SlideContent {
            index: idx as u32 + 1,
            paragraphs: parse_slide_xml(&xml_bytes),
        });
    }

    // Read slide dimensions after all slides — avoids any zip seek-order issue.
    let (slide_width_emu, slide_height_emu) =
        if let Ok(mut entry) = archive.by_name("ppt/presentation.xml") {
            let mut xml_bytes: Vec<u8> = Vec::new();
            match entry.read_to_end(&mut xml_bytes) {
                Ok(_) => parse_slide_size(&xml_bytes),
                Err(_) => (12192000u64, 6858000u64),
            }
        } else {
            (12192000u64, 6858000u64)
        };

    Ok(ParsedPresentation {
        file_path: file_path.to_string(),
        total_slides: total,
        slides,
        slide_width_emu,
        slide_height_emu,
    })
}

/// Parse a `.pptx` file into both the legacy text model and self-contained SVG slides.
/// Cached by `(file_path, modified_time, file_len)` so re-opening a deck with background photos is instant.
pub fn parse_pptx_svg(file_path: &str) -> Result<ParsedSvgPresentation, String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::SystemTime;

    type CacheKey = (String, Option<SystemTime>, u64);
    static DECK_CACHE: OnceLock<Mutex<HashMap<CacheKey, ParsedSvgPresentation>>> = OnceLock::new();

    let meta = std::fs::metadata(file_path).ok();
    let key: CacheKey = (
        file_path.to_string(),
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.as_ref().map(|m| m.len()).unwrap_or(0),
    );
    if key.2 > 0
        && let Some(cached) = DECK_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .ok()
            .and_then(|map| map.get(&key).cloned())
    {
        return Ok(cached);
    }

    let parsed_text = parse_pptx(file_path)?;
    let deck = presenter_svg::convert_pptx_to_inline_svg(Path::new(file_path))
        .map_err(|e| format!("Cannot convert presentation to SVG: {e}"))?;

    let parsed = ParsedSvgPresentation {
        file_path: file_path.to_string(),
        total_slides: deck.slides.len() as u32,
        slides: parsed_text.slides,
        svg_slides: deck
            .slides
            .into_iter()
            .map(|slide| SvgSlideContent {
                index: slide.index,
                svg: slide.svg,
                width_px: slide.width_px,
                height_px: slide.height_px,
            })
            .collect(),
        slide_width_emu: deck.width_emu,
        slide_height_emu: deck.height_emu,
    };
    if key.2 > 0
        && let Ok(mut map) = DECK_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock()
    {
        if map.len() >= 16 {
            map.clear();
        }
        map.insert(key, parsed.clone());
    }
    Ok(parsed)
}

pub fn load_pptx(
    file_path: &str,
    render_mode: PresenterRenderMode,
) -> Result<PresenterState, String> {
    let state = match render_mode {
        PresenterRenderMode::Text => parse_pptx(file_path).map(PresenterState::from_parsed),
        PresenterRenderMode::Svg => parse_pptx_svg(file_path).map(PresenterState::from_svg),
    }?;
    check_message_size(&state)?;
    Ok(state)
}

/// Room for the `{"type":"presenter.state","state":…}` envelope around the state.
const PRESENTER_STATE_ENVELOPE_BYTES: usize = 64;

/// The whole deck goes to every client as one `presenter.state` message, and the Rust
/// clients drop a message over `MAX_WS_MESSAGE_BYTES` along with the connection. Refuse such
/// a deck here, where the operator can be told why, instead of every client losing the server.
fn check_message_size(state: &PresenterState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state)
        .map_err(|e| format!("Cannot encode the presentation: {e}"))?
        .len()
        + PRESENTER_STATE_ENVELOPE_BYTES;
    if bytes > MAX_WS_MESSAGE_BYTES {
        return Err(format!(
            "This presentation is too large for the web presenter ({} MB, the limit is {} MB). \
             Compress its pictures in PowerPoint (Picture Format → Compress Pictures) and open it again.",
            bytes.div_ceil(1 << 20),
            MAX_WS_MESSAGE_BYTES >> 20,
        ));
    }
    Ok(())
}

// ── Live presenter state ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenterState {
    pub loaded: bool,
    pub file_path: Option<String>,
    pub current_slide: u32,
    pub total_slides: u32,
    pub render_mode: PresenterRenderMode,
    pub slides: Vec<SlideContent>,
    pub svg_slides: Vec<SvgSlideContent>,
    pub muted: bool,
    pub slide_width_emu: u64,
    pub slide_height_emu: u64,
}

impl PresenterState {
    pub fn empty() -> Self {
        Self {
            loaded: false,
            file_path: None,
            current_slide: 0,
            total_slides: 0,
            render_mode: PresenterRenderMode::Text,
            slides: Vec::new(),
            svg_slides: Vec::new(),
            muted: false,
            slide_width_emu: 12192000,
            slide_height_emu: 6858000,
        }
    }

    pub fn from_parsed(parsed: ParsedPresentation) -> Self {
        let total = parsed.total_slides;
        Self {
            loaded: true,
            file_path: Some(parsed.file_path),
            current_slide: if total > 0 { 1 } else { 0 },
            total_slides: total,
            render_mode: PresenterRenderMode::Text,
            slides: parsed.slides,
            svg_slides: Vec::new(),
            muted: false,
            slide_width_emu: parsed.slide_width_emu,
            slide_height_emu: parsed.slide_height_emu,
        }
    }

    pub fn from_svg(parsed: ParsedSvgPresentation) -> Self {
        let total = parsed.total_slides;
        Self {
            loaded: true,
            file_path: Some(parsed.file_path),
            current_slide: if total > 0 { 1 } else { 0 },
            total_slides: total,
            render_mode: PresenterRenderMode::Svg,
            slides: parsed.slides,
            svg_slides: parsed.svg_slides,
            muted: false,
            slide_width_emu: parsed.slide_width_emu,
            slide_height_emu: parsed.slide_height_emu,
        }
    }

    pub fn from_bible_reference(
        event_title: &str,
        reference_type: BibleReferenceType,
        reference: &str,
        verses: Vec<BibleVerseContent>,
    ) -> Self {
        Self::from_reading(
            event_title,
            reference_type.display_name(),
            reference,
            verses,
        )
    }

    /// A reading's verses as slides, `heading` naming it on each slide's counter (Textus,
    /// Lekció, or a Bible step's own title).
    pub fn from_reading(
        event_title: &str,
        heading: &str,
        reference: &str,
        verses: Vec<BibleVerseContent>,
    ) -> Self {
        let pages = paginate_bible_verses(verses, MAX_BIBLE_WORDS_PER_SLIDE);
        let total = pages.len() as u32;
        let paged = pages.iter().enumerate().map(|(idx, page)| {
            let verse_range = format_bible_page_range(page);
            let counter =
                format_bible_counter(heading, reference, &verse_range, idx as u32 + 1, total);
            vec![
                ParagraphContent {
                    lines: vec![format_bible_page_text(page)],
                    align: "left".to_string(),
                    font_size_pt: BIBLE_MAIN_FONT_SIZE_PT,
                },
                ParagraphContent {
                    lines: vec![counter],
                    align: "center".to_string(),
                    font_size_pt: BIBLE_COUNTER_FONT_SIZE_PT,
                },
            ]
        });
        // Bookended like a song deck; the counter keeps counting verse pages only.
        let slides = if total > 0 {
            let (heading, reference) = (heading.trim(), reference.trim());
            let title = if reference.is_empty() || reference == heading {
                heading.to_string()
            } else {
                format!("{heading} — {reference}")
            };
            with_bookends(&title, paged)
        } else {
            Vec::new()
        };
        let total = slides.len() as u32;

        Self {
            loaded: total > 0,
            file_path: Some(format!("{} - {} {}", event_title, heading, reference)),
            current_slide: if total > 0 { 1 } else { 0 },
            total_slides: total,
            render_mode: PresenterRenderMode::Text,
            slides,
            svg_slides: Vec::new(),
            muted: false,
            slide_width_emu: 12192000,
            slide_height_emu: 6858000,
        }
    }

    /// The announcements as slides: the title, then the bullets, a few to a slide so they stay
    /// readable from the back. With no bullets it is the title alone.
    pub fn from_announcements(title: &str, notes: &str) -> Self {
        let bullets: Vec<String> = notes
            .lines()
            .map(|line| line.trim().trim_start_matches(['•', '-', '*']).trim())
            .filter(|line| !line.is_empty())
            .map(|line| format!("• {line}"))
            .collect();
        let heading = |index| ParagraphContent {
            lines: vec![title.trim().to_string()],
            align: if index == 0 { "center" } else { "left" }.to_string(),
            font_size_pt: SONG_TITLE_FONT_SIZE_PT,
        };
        let slides: Vec<SlideContent> = if bullets.is_empty() {
            vec![SlideContent {
                index: 1,
                paragraphs: vec![heading(0)],
            }]
        } else {
            bullets
                .chunks(ANNOUNCEMENTS_PER_SLIDE)
                .enumerate()
                .map(|(index, page)| SlideContent {
                    index: index as u32 + 1,
                    paragraphs: vec![
                        heading(1),
                        ParagraphContent {
                            lines: page.iter().flat_map(|bullet| break_line(bullet)).collect(),
                            align: "left".to_string(),
                            font_size_pt: BIBLE_MAIN_FONT_SIZE_PT,
                        },
                    ],
                })
                .collect()
        };
        Self {
            loaded: true,
            file_path: Some(title.trim().to_string()),
            current_slide: 1,
            total_slides: slides.len() as u32,
            render_mode: PresenterRenderMode::Text,
            slides,
            svg_slides: Vec::new(),
            muted: false,
            slide_width_emu: 12192000,
            slide_height_emu: 6858000,
        }
    }

    /// One slide of its own, such as the welcome: the title, and each non-empty line of `notes`
    /// as a paragraph under it.
    pub fn from_text(title: &str, notes: &str) -> Self {
        let mut paragraphs = vec![ParagraphContent {
            lines: vec![title.trim().to_string()],
            align: "center".to_string(),
            font_size_pt: SONG_TITLE_FONT_SIZE_PT,
        }];
        let lines: Vec<String> = notes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .flat_map(break_line)
            .collect();
        if !lines.is_empty() {
            paragraphs.push(ParagraphContent {
                lines,
                align: "center".to_string(),
                font_size_pt: BIBLE_MAIN_FONT_SIZE_PT,
            });
        }
        Self {
            loaded: true,
            file_path: Some(title.trim().to_string()),
            current_slide: 1,
            total_slides: 1,
            render_mode: PresenterRenderMode::Text,
            slides: vec![SlideContent {
                index: 1,
                paragraphs,
            }],
            svg_slides: Vec::new(),
            muted: false,
            slide_width_emu: 12192000,
            slide_height_emu: 6858000,
        }
    }

    pub fn mute(&mut self) {
        self.muted = true;
    }

    pub fn unmute(&mut self) {
        self.muted = false;
    }

    pub fn go_next(&mut self) {
        if self.loaded && self.current_slide < self.total_slides {
            self.current_slide += 1;
        }
    }

    pub fn go_prev(&mut self) {
        if self.loaded && self.current_slide > 1 {
            self.current_slide -= 1;
        }
    }

    pub fn go_first(&mut self) {
        if self.loaded && self.total_slides > 0 {
            self.current_slide = 1;
        }
    }

    pub fn go_last(&mut self) {
        if self.loaded {
            self.current_slide = self.total_slides;
        }
    }

    pub fn go_to(&mut self, slide: u32) {
        if self.loaded && self.total_slides > 0 {
            self.current_slide = slide.max(1).min(self.total_slides);
        }
    }

    /// Replace the paragraphs of a slide from plain editor lines.
    ///
    /// Each text string becomes a single-line paragraph, preserving the
    /// original alignment and font size where possible.
    pub fn update_slide(&mut self, slide_index: u32, texts: Vec<String>) {
        if !self.loaded || self.render_mode != PresenterRenderMode::Text {
            return;
        }
        if let Some(slide) = self.slides.iter_mut().find(|s| s.index == slide_index) {
            let old = std::mem::take(&mut slide.paragraphs);
            slide.paragraphs = texts
                .into_iter()
                .enumerate()
                .map(|(i, text)| {
                    let old_para = old.get(i);
                    let align = old_para
                        .map(|p| p.align.as_str())
                        .unwrap_or("left")
                        .to_string();
                    let font_size_pt = old_para.map(|p| p.font_size_pt).unwrap_or(28.0);
                    ParagraphContent {
                        lines: vec![text],
                        align,
                        font_size_pt,
                    }
                })
                .collect();
        }
    }
}

fn paginate_bible_verses(
    verses: Vec<BibleVerseContent>,
    max_words_per_slide: usize,
) -> Vec<BibleSlidePage> {
    let max_words_per_slide = max_words_per_slide.max(1);
    let mut pages = Vec::new();
    let mut current_chunks = Vec::new();
    let mut current_word_count = 0usize;

    for verse in verses {
        let words = verse.text.split_whitespace().collect::<Vec<_>>();
        let word_count = words.len().max(1);

        if word_count <= max_words_per_slide {
            if current_word_count > 0 && current_word_count + word_count > max_words_per_slide {
                push_bible_page(&mut pages, &mut current_chunks, &mut current_word_count);
            }
            current_chunks.push(BibleVerseChunk {
                chapter: verse.chapter,
                verse: verse.verse,
                text: verse.text.trim().to_string(),
            });
            current_word_count += word_count;
            continue;
        }

        push_bible_page(&mut pages, &mut current_chunks, &mut current_word_count);

        for chunk_words in words.chunks(max_words_per_slide) {
            current_chunks.push(BibleVerseChunk {
                chapter: verse.chapter,
                verse: verse.verse,
                text: chunk_words.join(" "),
            });
            current_word_count = chunk_words.len();
            push_bible_page(&mut pages, &mut current_chunks, &mut current_word_count);
        }
    }

    push_bible_page(&mut pages, &mut current_chunks, &mut current_word_count);
    pages
}

fn push_bible_page(
    pages: &mut Vec<BibleSlidePage>,
    chunks: &mut Vec<BibleVerseChunk>,
    word_count: &mut usize,
) {
    if !chunks.is_empty() {
        pages.push(BibleSlidePage {
            chunks: std::mem::take(chunks),
        });
        *word_count = 0;
    }
}

fn format_bible_page_range(page: &BibleSlidePage) -> String {
    let Some(first) = page.chunks.first() else {
        return String::new();
    };
    let last = page.chunks.last().unwrap_or(first);
    if first.chapter == last.chapter && first.verse == last.verse {
        format!("{}:{}", first.chapter, first.verse)
    } else if first.chapter == last.chapter {
        format!("{}:{}-{}", first.chapter, first.verse, last.verse)
    } else {
        format!(
            "{}:{}-{}:{}",
            first.chapter, first.verse, last.chapter, last.verse
        )
    }
}

fn format_bible_page_text(page: &BibleSlidePage) -> String {
    page.chunks
        .iter()
        .map(|chunk| chunk.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_bible_counter(
    display_name: &str,
    reference: &str,
    verse_range: &str,
    slide_number: u32,
    total: u32,
) -> String {
    let trimmed_reference = reference.trim();
    if trimmed_reference.is_empty() {
        format!("{display_name} {verse_range} ({slide_number}/{total})")
    } else {
        format!("{display_name} {trimmed_reference} | {verse_range} ({slide_number}/{total})")
    }
}

/// Breaks a line too long for the song size, at the last punctuation that fits, or else the last
/// space. A word too long to break stays whole, and so does a paragraph.
fn split_song_line(line: &str) -> Vec<String> {
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > SONG_PARAGRAPH_CHARS {
        return vec![line];
    }
    break_line(&line)
}

/// Breaks text into lines of at most `SONG_LINE_CHARS`, at the last punctuation that fits, or else
/// the last space; a word too long to break stays whole. Renderers never wrap a line, so generated
/// prose (announcements, the welcome) is broken here however long it is.
fn break_line(line: &str) -> Vec<String> {
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts = Vec::new();
    let mut rest = line.as_str();
    while rest.chars().count() > SONG_LINE_CHARS {
        let mut last_space = None;
        let mut last_phrase = None;
        let mut previous = ' ';
        for (position, (index, ch)) in rest.char_indices().enumerate() {
            if position > SONG_LINE_CHARS {
                break;
            }
            if ch == ' ' {
                last_space = Some(index);
                if position >= SONG_LINE_CHARS / 3 && ",;:.!?".contains(previous) {
                    last_phrase = Some(index);
                }
            }
            previous = ch;
        }
        let Some(index) = last_phrase.or(last_space) else {
            break;
        };
        parts.push(rest[..index].to_string());
        rest = &rest[index + 1..];
    }
    parts.push(rest.to_string());
    parts
}

fn finish_song_page(pages: &mut Vec<Vec<String>>, lines: &mut Vec<String>) {
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.iter().any(|line| !line.is_empty()) {
        pages.push(std::mem::take(lines));
    }
}

/// Two empty input lines separate slides. One empty line stays inside the slide.
pub fn song_slides(title: &str, lyrics: &str) -> Vec<SlideContent> {
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut lines: Vec<String> = Vec::new();

    for line in lyrics.lines() {
        let line = line.trim();
        if line.is_empty() {
            finish_song_page(&mut pages, &mut lines);
            continue;
        }
        let lowercase = line.to_lowercase();
        if ["refr.", "refr:", "refrén"]
            .iter()
            .any(|refrain| lowercase.starts_with(refrain))
        {
            finish_song_page(&mut pages, &mut lines);
        }
        lines.extend(split_song_line(line));
    }
    finish_song_page(&mut pages, &mut lines);

    with_bookends(
        title,
        pages.into_iter().map(|lines| {
            vec![ParagraphContent {
                lines,
                align: "center".to_string(),
                font_size_pt: SONG_FONT_SIZE_PT,
            }]
        }),
    )
}

/// Wraps a deck's pages between a centred title slide and a blank closing slide.
fn with_bookends(
    title: &str,
    pages: impl IntoIterator<Item = Vec<ParagraphContent>>,
) -> Vec<SlideContent> {
    let mut slides = vec![SlideContent {
        index: 1,
        paragraphs: vec![ParagraphContent {
            lines: vec![title.trim().to_string()],
            align: "center".to_string(),
            font_size_pt: SONG_TITLE_FONT_SIZE_PT,
        }],
    }];
    slides.extend(
        pages
            .into_iter()
            .enumerate()
            .map(|(index, paragraphs)| SlideContent {
                index: index as u32 + 2,
                paragraphs,
            }),
    );
    slides.push(SlideContent {
        index: slides.len() as u32 + 1,
        paragraphs: Vec::new(),
    });
    slides
}

/// Keep the user-facing title while preventing it from escaping the output folder.
pub fn song_file_name(title: &str) -> Option<String> {
    let cleaned = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stem = cleaned.chars().take(80).collect::<String>();
    (!stem.is_empty()).then(|| format!("{stem}.pptx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verse(chapter: u32, verse: u32, text: &str) -> BibleVerseContent {
        BibleVerseContent {
            chapter,
            verse,
            text: text.to_string(),
        }
    }

    #[test]
    fn a_deck_too_large_for_one_websocket_message_is_refused() {
        let svg_slide = |svg: String| SvgSlideContent {
            index: 1,
            svg,
            width_px: 1920,
            height_px: 1080,
        };
        let mut state = PresenterState::empty();
        state.svg_slides = vec![svg_slide("x".repeat(MAX_WS_MESSAGE_BYTES))];
        let error = check_message_size(&state).unwrap_err();
        assert!(error.contains("too large"), "{error}");

        state.svg_slides = vec![svg_slide("x".repeat(20 << 20))];
        assert!(check_message_size(&state).is_ok());
    }

    #[test]
    fn presenter_theme_parses_persisted_values() {
        assert_eq!("classic".parse(), Ok(PresenterTheme::Classic));
        assert_eq!("editorial".parse(), Ok(PresenterTheme::Editorial));
        assert!("unknown".parse::<PresenterTheme>().is_err());
    }

    #[test]
    fn a_pasted_song_keeps_the_song_size_on_every_slide() {
        let slides = song_slides(
            "Erőt adsz",
            "1. Erőt adsz minden helyzetben,\nTe vagy a kincsem, életem, Te vagy a mindenem.\n\
             Kereslek, mert oly drága vagy,\nNincs mivel pótolhatnálak, Te vagy a mindenem.\n\n\
             Refr.: Jézus, hű Bárány, áldom szent Neved.\n\
             Jézus, hű Bárány, áldom szent Neved.\n\n\
             2. Elvetted bűnöm, szégyenem,\nKöszönöm, hogy újjá lettem, Te vagy a mindenem.\n\
             Felemelsz, hogyha elbukom,\nSzentlelked betölt, jól tudom.  Te vagy a mindenem.\n\
             Refr.: Jézus, hű Bárány, áldom szent Neved.\n\
             Jézus, hű Bárány, áldom szent Neved.",
        );

        assert_eq!(slides.len(), 6, "title, four sections and the blank slide");
        assert_eq!(
            slides[1].paragraphs[0].lines[1..3],
            ["Te vagy a kincsem, életem,", "Te vagy a mindenem."]
        );
        let margin = 12_192_000 / 40;
        for slide in &slides[1..5] {
            let lines = &slide.paragraphs[0].lines;
            let (size, wrap) = song_text_fit(
                lines,
                12_192_000 - 2 * margin,
                6_858_000 - 2 * (6_858_000 / 40),
                SONG_FONT_SIZE_PT,
            );
            assert_eq!(size, SONG_FONT_SIZE_PT, "{lines:?}");
            assert_eq!(wrap, "none");
        }
    }

    #[test]
    fn song_sections_become_safe_paged_slides() {
        let slides = song_slides(
            "Song title",
            "first line\nsame slide\n\nnext slide\n\n\nafter two blanks\nRefr.: refrain slide",
        );

        assert_eq!(slides.len(), 6);
        assert_eq!(slides[0].paragraphs[0].lines, ["Song title"]);
        assert_eq!(slides[1].paragraphs[0].lines, ["first line", "same slide"]);
        assert_eq!(slides[2].paragraphs[0].lines, ["next slide"]);
        assert_eq!(slides[3].paragraphs[0].lines, ["after two blanks"]);
        assert_eq!(slides[4].paragraphs[0].lines, ["Refr.: refrain slide"]);
        assert!(slides[5].paragraphs.is_empty());
        assert_eq!(
            song_file_name("../Kegyelem: 1/2"),
            Some("Kegyelem 1 2.pptx".into())
        );
    }

    #[test]
    fn paginates_short_verses_without_splitting_them() {
        let pages = paginate_bible_verses(
            vec![
                verse(3, 16, "one two three"),
                verse(3, 17, "four five"),
                verse(3, 18, "six seven eight"),
            ],
            5,
        );

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].chunks.len(), 2);
        assert_eq!(format_bible_page_range(&pages[0]), "3:16-17");
        assert_eq!(pages[1].chunks[0].verse, 18);
    }

    #[test]
    fn splits_single_oversized_verse_by_word_limit() {
        let pages = paginate_bible_verses(vec![verse(4, 1, "one two three four five six")], 2);

        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].chunks[0].text, "one two");
        assert_eq!(pages[1].chunks[0].text, "three four");
        assert_eq!(pages[2].chunks[0].text, "five six");
        assert_eq!(format_bible_page_range(&pages[2]), "4:1");
    }

    #[test]
    fn bible_reference_slide_joins_verses_into_one_plain_paragraph() {
        let state = PresenterState::from_bible_reference(
            "Sunday",
            BibleReferenceType::Textus,
            "Jn 3:16-17",
            vec![
                verse(3, 16, "For God so loved the world."),
                verse(3, 17, "God did not send the Son to condemn the world."),
            ],
        );

        let slide = &state.slides[1];
        assert_eq!(slide.paragraphs.len(), 2);
        assert_eq!(slide.paragraphs[0].lines.len(), 1);
        assert_eq!(
            slide.paragraphs[0].lines.join(" "),
            "For God so loved the world. God did not send the Son to condemn the world."
        );
        assert_eq!(slide.paragraphs[0].align, "left");
        assert!(slide.paragraphs[1].lines[0].contains("Jn 3:16-17"));
    }

    #[test]
    fn bible_reading_is_bookended_like_a_song() {
        let state = PresenterState::from_bible_reference(
            "Sunday",
            BibleReferenceType::Leckio,
            "Jn 3:16-17",
            vec![verse(3, 16, "one two"), verse(3, 17, "three four")],
        );
        let song = song_slides("Song title", "line");

        assert_eq!(state.total_slides, 3);
        assert_eq!(state.slides.len(), 3);
        assert_eq!(state.slides[0].paragraphs[0].lines, ["Lekció — Jn 3:16-17"]);
        assert_eq!(state.slides[0].paragraphs.len(), song[0].paragraphs.len());
        assert_eq!(
            state.slides[0].paragraphs[0].align,
            song[0].paragraphs[0].align
        );
        assert_eq!(
            state.slides[0].paragraphs[0].font_size_pt,
            song[0].paragraphs[0].font_size_pt
        );
        assert!(state.slides[1].paragraphs[1].lines[0].ends_with("(1/1)"));
        assert!(state.slides[2].paragraphs.is_empty());
        assert_eq!(
            state
                .slides
                .iter()
                .map(|slide| slide.index)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );

        let empty = PresenterState::from_reading("Sunday", "Jn 3:16", "Jn 3:16", Vec::new());
        assert!(!empty.loaded);
        assert!(empty.slides.is_empty());
        let own =
            PresenterState::from_reading("Sunday", "Jn 3:16", "Jn 3:16", vec![verse(3, 16, "x")]);
        assert_eq!(own.slides[0].paragraphs[0].lines, ["Jn 3:16"]);
    }

    #[test]
    fn written_pptx_reparses_to_the_same_slides() {
        let state = PresenterState::from_bible_reference(
            "Sunday",
            BibleReferenceType::Textus,
            "Jn 3:16",
            vec![verse(3, 16, "For God so loved the world.")],
        );
        let path = std::env::temp_dir().join(format!("metocast-test-{}.pptx", std::process::id()));
        write_pptx(
            &path,
            &state.slides,
            state.slide_width_emu,
            state.slide_height_emu,
            PresenterTheme::Classic,
        )
        .unwrap();

        let parsed = parse_pptx(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(parsed.total_slides, state.total_slides);
        assert_eq!(parsed.slide_width_emu, state.slide_width_emu);
        let written = &parsed.slides[1].paragraphs;
        let expected = &state.slides[1].paragraphs;
        assert_eq!(written.len(), expected.len());
        assert_eq!(written[0].lines, expected[0].lines);
        assert_eq!(written[0].align, expected[0].align);
        assert_eq!(written[0].font_size_pt, expected[0].font_size_pt);
        assert_eq!(written[1].align, "center");
    }

    #[test]
    fn editorial_pptx_keeps_semantic_bible_text() {
        let state = PresenterState::from_bible_reference(
            "Sunday",
            BibleReferenceType::Textus,
            "Jn 3:16",
            vec![verse(3, 16, "For God so loved the world.")],
        );
        let path = std::env::temp_dir().join(format!(
            "metocast-editorial-test-{}.pptx",
            std::process::id()
        ));
        write_pptx(
            &path,
            &state.slides,
            state.slide_width_emu,
            state.slide_height_emu,
            PresenterTheme::Editorial,
        )
        .unwrap();

        let parsed = parse_pptx(path.to_str().unwrap()).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide2.xml").unwrap(),
            &mut xml,
        )
        .unwrap();
        std::fs::remove_file(&path).ok();

        assert!(xml.contains("11100E"));
        assert!(xml.contains("Cormorant Garamond"));
        assert!(xml.contains("Metocast Decorative Verse Number"));
        assert_eq!(parsed.slides[1].paragraphs.len(), 2);
        assert_eq!(
            parsed.slides[1].paragraphs[0].lines,
            state.slides[1].paragraphs[0].lines
        );
        assert_eq!(
            parsed.slides[1].paragraphs[1].lines,
            state.slides[1].paragraphs[1].lines
        );
    }

    /// PowerPoint offers to "repair" a deck when a `p:txBody` has no `a:p`.
    fn assert_text_bodies_have_paragraphs(xml: &str) {
        use quick_xml::{Reader, events::Event};

        let mut reader = Reader::from_str(xml);
        let mut in_body = false;
        let mut has_paragraph = false;
        loop {
            match reader.read_event().unwrap() {
                Event::Start(e) => match e.name().into_inner() {
                    b"p:txBody" => {
                        in_body = true;
                        has_paragraph = false;
                    }
                    b"a:p" if in_body => has_paragraph = true,
                    _ => {}
                },
                Event::End(e) if e.name().into_inner() == b"p:txBody" => {
                    assert!(has_paragraph, "p:txBody without a:p in {xml}");
                    in_body = false;
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }

    #[test]
    fn a_text_step_is_one_slide_with_its_paragraphs() {
        let deck = PresenterState::from_text(
            "Köszöntjük!",
            "Isten hozott mindenkit\n\nKérjük, némítsa le telefonját",
        );
        assert_eq!(deck.total_slides, 1);
        let paragraphs = &deck.slides[0].paragraphs;
        assert_eq!(paragraphs[0].lines, ["Köszöntjük!"]);
        assert_eq!(
            paragraphs[1].lines,
            ["Isten hozott mindenkit", "Kérjük, némítsa le telefonját"]
        );
        assert_eq!(
            PresenterState::from_text("Köszöntjük!", "").slides[0]
                .paragraphs
                .len(),
            1
        );
    }

    #[test]
    fn long_announcement_bullets_wrap_into_short_lines() {
        let bullet = "Jövő vasárnap a gyülekezeti kiránduláson, a hegyen, közös ebéd lesz; mindenkit szeretettel várunk, a jelentkezéseket a lelkészi hivatalban fogadjuk, hétfőtől péntekig, délelőtt kilenc és tizenkét óra között";
        let notes = format!("{bullet}\nKonfirmáció\nTemetés\nPresbiteri gyűlés\nBibliaóra");
        let deck = PresenterState::from_announcements("Hirdetések", &notes);
        assert_eq!(deck.total_slides, 1, "a wrapped bullet still counts once");
        let lines = &deck.slides[0].paragraphs[1].lines;
        assert!(lines.len() > 5 + 2, "{lines:?}");
        assert!(lines[0].starts_with("• Jövő"));
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() <= SONG_LINE_CHARS),
            "{lines:?}"
        );
        assert_eq!(
            lines.join(" "),
            format!("• {bullet} • Konfirmáció • Temetés • Presbiteri gyűlés • Bibliaóra")
        );

        let welcome = PresenterState::from_text("Köszöntjük!", bullet);
        assert!(
            welcome.slides[0].paragraphs[1]
                .lines
                .iter()
                .all(|line| line.chars().count() <= SONG_LINE_CHARS)
        );
    }

    #[test]
    fn a_long_reading_takes_more_slides_than_it_used_to() {
        let ten_words = "egy kettő három négy öt hat hét nyolc kilenc tíz";
        let reading = || {
            (1..=7)
                .map(|number| verse(5, number, ten_words))
                .collect::<Vec<_>>()
        };
        assert_eq!(paginate_bible_verses(reading(), 55).len(), 2);
        let pages = paginate_bible_verses(reading(), MAX_BIBLE_WORDS_PER_SLIDE);
        assert_eq!(pages.len(), 3);
        assert_eq!(format_bible_page_range(&pages[0]), "5:1-3");
    }

    #[test]
    fn announcements_become_bullet_slides() {
        let notes = "Bibliaóra csütörtökön\n\n- Ifjúsági alkalom\n• Gyülekezeti kirándulás\nPresbiteri gyűlés\nKonfirmáció\nTemetés";
        let deck = PresenterState::from_announcements("Hirdetések", notes);
        assert_eq!(deck.total_slides, 2);
        let first = &deck.slides[0].paragraphs;
        assert_eq!(first[0].lines, ["Hirdetések"]);
        assert_eq!(first[1].lines[1], "• Ifjúsági alkalom");
        assert_eq!(first[1].lines[2], "• Gyülekezeti kirándulás");
        assert_eq!(deck.slides[1].paragraphs[1].lines, ["• Temetés"]);

        let empty = PresenterState::from_announcements("Hirdetések, áldás", " \n");
        assert_eq!(empty.total_slides, 1);
        assert_eq!(empty.slides[0].paragraphs.len(), 1);
    }

    #[test]
    fn song_decks_never_write_an_empty_text_body() {
        let slides = song_slides("Song title", "First line\nSecond line\n\n\nChorus line");
        for (label, theme) in [
            ("classic", PresenterTheme::Classic),
            ("editorial", PresenterTheme::Editorial),
        ] {
            let path = std::env::temp_dir().join(format!(
                "metocast-{label}-empty-body-test-{}.pptx",
                std::process::id()
            ));
            write_pptx(&path, &slides, 12_192_000, 6_858_000, theme).unwrap();
            let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
            for number in 1..=slides.len() {
                let mut xml = String::new();
                std::io::Read::read_to_string(
                    &mut archive
                        .by_name(&format!("ppt/slides/slide{number}.xml"))
                        .unwrap(),
                    &mut xml,
                )
                .unwrap();
                assert_text_bodies_have_paragraphs(&xml);
            }
            drop(archive);
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn only_paragraphs_wrap_and_lyric_lines_never_do() {
        let long = vec!["word ".repeat(46).trim().to_string()];
        let (size, wrap) = song_text_fit(&long, 10_160_000, 5_715_000, SONG_FONT_SIZE_PT);
        assert_eq!(wrap, "square");
        assert!(
            size >= 24.0,
            "wrapped text should stay readable, got {size}"
        );

        let short = vec!["A short lyric line".to_string()];
        let (size, wrap) = song_text_fit(&short, 10_160_000, 5_715_000, SONG_FONT_SIZE_PT);
        assert_eq!(wrap, "none");
        assert_eq!(size, SONG_FONT_SIZE_PT);

        // Up to 150 characters a line is a lyric, however long: it never wraps.
        let lyric = vec!["a".repeat(150)];
        let (size, wrap) = song_text_fit(&lyric, 10_160_000, 5_715_000, SONG_FONT_SIZE_PT);
        assert_eq!(wrap, "none");
        assert!(
            size < 24.0,
            "an unwrapped lyric keeps shrinking to fit, got {size}"
        );
        let paragraph = vec!["a".repeat(151)];
        let (_, wrap) = song_text_fit(&paragraph, 10_160_000, 5_715_000, SONG_FONT_SIZE_PT);
        assert_eq!(wrap, "square");

        let slides = song_slides("Title", &long[0]);
        let xml = classic_slide_xml(&slides[1], 12_192_000, 6_858_000);
        assert!(xml.contains(r#"wrap="square""#));
        assert_eq!(
            parse_slide_xml(xml.as_bytes())[0].lines,
            long,
            "wrapping must not split the entered line"
        );
    }

    #[test]
    fn classic_slides_name_their_font() {
        let slides = song_slides("Song title", "A line");
        let xml = classic_slide_xml(&slides[1], 12_192_000, 6_858_000);
        assert!(xml.contains(r#"<a:latin typeface="Helvetica"/>"#));
    }

    #[test]
    fn editorial_song_pptx_keeps_lines_and_bookends() {
        let slides = song_slides("Song title", "This lyric stays on one line\nsecond line");
        let path = std::env::temp_dir().join(format!(
            "metocast-editorial-song-test-{}.pptx",
            std::process::id()
        ));
        write_pptx(
            &path,
            &slides,
            12_192_000,
            6_858_000,
            PresenterTheme::Editorial,
        )
        .unwrap();

        let parsed = parse_pptx(path.to_str().unwrap()).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide2.xml").unwrap(),
            &mut xml,
        )
        .unwrap();
        drop(archive);
        std::fs::remove_file(&path).ok();

        assert_eq!(parsed.total_slides, 3);
        assert_eq!(parsed.slides[0].paragraphs[0].lines, ["Song title"]);
        assert_eq!(
            parsed.slides[1].paragraphs[0].lines,
            ["This lyric stays on one line", "second line"]
        );
        assert!(parsed.slides[2].paragraphs.is_empty());
        assert!(xml.contains(r#"wrap="none""#));
    }
}

// ── HTTP handler ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseBody {
    pub file_path: String,
}

pub async fn parse_presentation(
    State(_state): State<AppState>,
    Json(body): Json<ParseBody>,
) -> impl IntoResponse {
    let file_path = body.file_path;
    let result = tokio::task::spawn_blocking(move || parse_pptx(&file_path)).await;

    match result {
        Ok(Ok(parsed)) => (
            StatusCode::OK,
            Json(json!({ "success": true, "data": parsed })),
        ),
        Ok(Err(e)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "success": false, "error": e })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "success": false, "error": e.to_string() })),
        ),
    }
}

// ── PPTX writing ──────────────────────────────────────────────────────────────

const THEME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Metocast"><a:themeElements><a:clrScheme name="Metocast"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="000000"/></a:dk2><a:lt2><a:srgbClr val="FFFFFF"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Metocast"><a:majorFont><a:latin typeface="Helvetica Neue"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Helvetica"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Metocast"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

const REL_NS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#;

/// `algn` attribute value for a CSS alignment keyword — the inverse of `map_align`.
fn pptx_align(css: &str) -> &'static str {
    match css {
        "center" => "ctr",
        "right" => "r",
        "justify" => "just",
        _ => "l",
    }
}

fn classic_slide_xml(slide: &SlideContent, width_emu: u64, height_emu: u64) -> String {
    let is_bible = slide_counter_index(slide).is_some();
    let margin_divisor = if is_bible { 12 } else { 40 };
    let margin_x = width_emu / margin_divisor;
    let margin_y = height_emu / margin_divisor;
    let fits: Vec<(f32, &str)> = slide
        .paragraphs
        .iter()
        .map(|p| {
            if is_bible {
                (p.font_size_pt, "square")
            } else {
                song_text_fit(
                    &p.lines,
                    width_emu - 2 * margin_x,
                    height_emu - 2 * margin_y,
                    p.font_size_pt,
                )
            }
        })
        .collect();
    let wrap = if fits.iter().any(|(_, wrap)| *wrap == "square") {
        "square"
    } else {
        "none"
    };
    let paragraphs = slide
        .paragraphs
        .iter()
        .zip(&fits)
        .map(|(p, (font_size_pt, _))| {
            let sz = (font_size_pt * 100.0).round().max(100.0) as u32;
            let algn = pptx_align(&p.align);
            let runs = p
                .lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    format!(
                        r#"{}<a:r><a:rPr lang="hu-HU" sz="{sz}" b="1" dirty="0"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Helvetica"/></a:rPr><a:t>{}</a:t></a:r>"#,
                        if i > 0 { "<a:br/>" } else { "" },
                        quick_xml::escape::escape(line),
                    )
                })
                .collect::<String>();
            format!(r#"<a:p><a:pPr algn="{algn}"/>{runs}</a:p>"#)
        })
        .collect::<String>();

    // A `txBody` must hold at least one `a:p`, so a blank slide gets no body shape.
    let body = if slide.paragraphs.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Body"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{margin_x}" y="{margin_y}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr wrap="{wrap}" anchor="ctr"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp>"#,
            width_emu - 2 * margin_x,
            height_emu - 2 * margin_y,
        )
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="000000"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{EMPTY_TREE}{body}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn slide_counter_index(slide: &SlideContent) -> Option<usize> {
    if slide.paragraphs.len() < 2 {
        return None;
    }
    let max_size = slide
        .paragraphs
        .iter()
        .map(|paragraph| paragraph.font_size_pt)
        .fold(0.0_f32, f32::max);
    let is_counter = |index: usize| {
        let paragraph = &slide.paragraphs[index];
        paragraph.font_size_pt > 0.0
            && paragraph.font_size_pt < max_size * 0.85
            && paragraph.align == "center"
    };
    let last = slide.paragraphs.len() - 1;
    is_counter(0)
        .then_some(0)
        .or_else(|| is_counter(last).then_some(last))
}

struct TextBoxStyle<'a> {
    font: &'a str,
    font_size_pt: f32,
    color: &'a str,
    align: &'a str,
    anchor: &'a str,
    wrap: &'a str,
    bold: bool,
}

fn text_box_xml(
    id: u32,
    name: &str,
    bounds: (u64, u64, u64, u64),
    lines: &[String],
    style: TextBoxStyle<'_>,
) -> String {
    let TextBoxStyle {
        font,
        font_size_pt,
        color,
        align,
        anchor,
        wrap,
        bold,
    } = style;
    let (x, y, width, height) = bounds;
    let size = (font_size_pt * 100.0).round().max(100.0) as u32;
    let runs = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                r#"{}<a:r><a:rPr lang="hu-HU" sz="{size}" b="{}" dirty="0"><a:solidFill><a:srgbClr val="{color}"/></a:solidFill><a:latin typeface="{font}"/><a:ea typeface="{font}"/><a:cs typeface="{font}"/></a:rPr><a:t>{}</a:t></a:r>"#,
                if index > 0 { "<a:br/>" } else { "" },
                u8::from(bold),
                quick_xml::escape::escape(line),
            )
        })
        .collect::<String>();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr><p:txBody><a:bodyPr wrap="{wrap}" anchor="{anchor}" lIns="0" rIns="0" tIns="0" bIns="0"><a:normAutofit/></a:bodyPr><a:lstStyle/><a:p><a:pPr algn="{}"/>{runs}</a:p></p:txBody></p:sp>"#,
        quick_xml::escape::escape(name),
        pptx_align(align),
    )
}

fn bible_label(counter: &str) -> String {
    counter
        .rsplit_once(" (")
        .map_or(counter, |(label, _)| label)
        .replace(" | ", " · ")
        .to_uppercase()
}

fn bible_verse_number(counter: &str) -> String {
    counter
        .split_whitespace()
        .filter_map(|word| word.split_once(':').map(|(_, verse)| verse))
        .map(|verse| {
            verse
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .rfind(|verse| !verse.is_empty())
        .unwrap_or_default()
}

/// Font size and `wrap` mode for song text. Lines stay on one line, since
/// `song_slides` has already broken the long ones. The one exception is a paragraph: a line over `PARAGRAPH_CHARS` wraps inside the box
/// and shrinks only until the wrapped block fits, instead of becoming a 12 pt
/// strip running off both edges of the slide.
fn song_text_fit(
    lines: &[String],
    width_emu: u64,
    height_emu: u64,
    preferred: f32,
) -> (f32, &'static str) {
    const EMU_PER_POINT: f32 = 12_700.0;
    const MIN_PT: f32 = 12.0;
    const CHAR_WIDTH_EM: f32 = 0.54;
    // Word wrapping leaves ragged line ends, so wrapped text needs wider estimates.
    const WRAPPED_CHAR_WIDTH_EM: f32 = 0.6;
    const LINE_HEIGHT_EM: f32 = 1.2;

    let width_pt = width_emu as f32 / EMU_PER_POINT;
    let height_pt = height_emu as f32 / EMU_PER_POINT;
    let char_counts: Vec<f32> = lines
        .iter()
        .map(|line| line.chars().count().max(1) as f32)
        .collect();
    let longest = char_counts.iter().copied().fold(1.0_f32, f32::max);
    let line_count = char_counts.len().max(1) as f32;

    let width_limit = width_pt / (longest * CHAR_WIDTH_EM);
    let height_limit = height_pt / (line_count * LINE_HEIGHT_EM);
    if longest <= SONG_PARAGRAPH_CHARS as f32 {
        return (
            preferred.min(width_limit).min(height_limit).max(MIN_PT),
            "none",
        );
    }

    let mut size = preferred.max(MIN_PT);
    while size > MIN_PT {
        let rows: f32 = char_counts
            .iter()
            .map(|chars| {
                (chars * size * WRAPPED_CHAR_WIDTH_EM / width_pt)
                    .ceil()
                    .max(1.0)
            })
            .sum();
        if rows * size * LINE_HEIGHT_EM <= height_pt {
            break;
        }
        size -= 1.0;
    }
    (size, "square")
}

fn editorial_slide_xml(
    slide: &SlideContent,
    deck_label: &str,
    width_emu: u64,
    height_emu: u64,
) -> String {
    if slide.paragraphs.is_empty() {
        return format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="11100E"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        );
    }
    let counter_index = slide_counter_index(slide);
    let counter = counter_index
        .and_then(|index| slide.paragraphs.get(index))
        .map(|paragraph| paragraph.lines.join(" "));
    let main = slide
        .paragraphs
        .iter()
        .enumerate()
        .find(|(index, _)| Some(*index) != counter_index)
        .map(|(_, paragraph)| paragraph)
        .cloned()
        .unwrap_or(ParagraphContent {
            lines: Vec::new(),
            align: "center".to_string(),
            font_size_pt: SONG_FONT_SIZE_PT,
        });
    let is_bible = counter
        .as_deref()
        .is_some_and(|text| text.starts_with("Textus") || text.starts_with("Lekció"));
    let is_title = !is_bible && slide.index == 1;
    let text_length = main.lines.iter().map(String::len).sum::<usize>();
    let song_fit = (!is_bible).then(|| {
        song_text_fit(
            &main.lines,
            width_emu * 96 / 100,
            height_emu * 68 / 100,
            if is_title {
                SONG_TITLE_FONT_SIZE_PT
            } else {
                46.0
            },
        )
    });
    let song_wrap = song_fit.map_or("none", |(_, wrap)| wrap);
    let body_size = if let Some((size, _)) = song_fit {
        size
    } else if text_length > 300 {
        30.0
    } else if text_length > 180 {
        36.0
    } else {
        46.0
    };

    let shapes = if is_bible {
        let counter = counter.unwrap_or_default();
        let label = bible_label(&counter);
        let verse_number = bible_verse_number(&counter);
        let hidden_counter = slide.paragraphs[counter_index.unwrap_or_default()].clone();
        [
            text_box_xml(
                2,
                "Body",
                (
                    width_emu * 29 / 100,
                    height_emu * 31 / 100,
                    width_emu * 62 / 100,
                    height_emu * 50 / 100,
                ),
                &main.lines,
                TextBoxStyle {
                    font: "Cormorant Garamond",
                    font_size_pt: body_size,
                    color: "F4EFE3",
                    align: "left",
                    anchor: "t",
                    wrap: "square",
                    bold: false,
                },
            ),
            text_box_xml(
                3,
                "Metocast Decorative Reference",
                (
                    width_emu * 29 / 100,
                    height_emu * 22 / 100,
                    width_emu * 62 / 100,
                    height_emu * 8 / 100,
                ),
                &[label],
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: 11.0,
                    color: "CFAB69",
                    align: "left",
                    anchor: "b",
                    wrap: "none",
                    bold: false,
                },
            ),
            text_box_xml(
                4,
                "Metocast Decorative Verse Number",
                (
                    width_emu * 5 / 100,
                    height_emu * 31 / 100,
                    width_emu * 20 / 100,
                    height_emu * 34 / 100,
                ),
                &[verse_number],
                TextBoxStyle {
                    font: "Cormorant Garamond",
                    font_size_pt: 126.0,
                    color: "CFAB69",
                    align: "right",
                    anchor: "t",
                    wrap: "none",
                    bold: false,
                },
            ),
            text_box_xml(
                5,
                "Metocast Decorative Mark",
                (
                    width_emu * 84 / 100,
                    height_emu * 90 / 100,
                    width_emu * 12 / 100,
                    height_emu * 4 / 100,
                ),
                &["METOCAST".to_string()],
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: 8.0,
                    color: "77736B",
                    align: "right",
                    anchor: "ctr",
                    wrap: "none",
                    bold: false,
                },
            ),
            text_box_xml(
                6,
                "Metocast Semantic Counter",
                (width_emu + 1, 0, width_emu / 5, height_emu / 10),
                &hidden_counter.lines,
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: hidden_counter.font_size_pt,
                    color: "11100E",
                    align: "center",
                    anchor: "ctr",
                    wrap: "none",
                    bold: false,
                },
            ),
        ]
        .join("")
    } else {
        let is_reading_title = main
            .lines
            .first()
            .is_some_and(|text| text.starts_with("Textus") || text.starts_with("Lekció"));
        let label = if is_title && is_reading_title {
            "IGE".to_string()
        } else if is_title {
            "ÉNEK".to_string()
        } else {
            format!("ÉNEK · {deck_label}").to_uppercase()
        };
        let mut song_shapes = vec![
            text_box_xml(
                2,
                "Body",
                (
                    width_emu * 2 / 100,
                    height_emu * 20 / 100,
                    width_emu * 96 / 100,
                    height_emu * 68 / 100,
                ),
                &main.lines,
                TextBoxStyle {
                    font: "Cormorant Garamond",
                    font_size_pt: body_size,
                    color: "F4EFE3",
                    align: "center",
                    anchor: "ctr",
                    wrap: song_wrap,
                    bold: false,
                },
            ),
            text_box_xml(
                3,
                "Metocast Decorative Song Label",
                (
                    width_emu * 10 / 100,
                    height_emu * 12 / 100,
                    width_emu * 80 / 100,
                    height_emu * 7 / 100,
                ),
                &[label],
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: 11.0,
                    color: "CFAB69",
                    align: "center",
                    anchor: "b",
                    wrap: "none",
                    bold: false,
                },
            ),
            text_box_xml(
                5,
                "Metocast Decorative Mark",
                (
                    width_emu * 84 / 100,
                    height_emu * 90 / 100,
                    width_emu * 12 / 100,
                    height_emu * 4 / 100,
                ),
                &["METOCAST".to_string()],
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: 8.0,
                    color: "77736B",
                    align: "right",
                    anchor: "ctr",
                    wrap: "none",
                    bold: false,
                },
            ),
        ];
        if !is_title {
            song_shapes.push(text_box_xml(
                4,
                "Metocast Decorative Slide Number",
                (
                    width_emu * 40 / 100,
                    height_emu * 82 / 100,
                    width_emu * 20 / 100,
                    height_emu * 5 / 100,
                ),
                &[format!("{:02}. DIA", slide.index)],
                TextBoxStyle {
                    font: "Geist Mono",
                    font_size_pt: 9.0,
                    color: "CFAB69",
                    align: "center",
                    anchor: "ctr",
                    wrap: "none",
                    bold: false,
                },
            ));
        }
        song_shapes.join("")
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="11100E"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{EMPTY_TREE}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

/// Write `slides` as a minimal but valid `.pptx` package in the selected design.
pub fn write_pptx(
    path: &Path,
    slides: &[SlideContent],
    width_emu: u64,
    height_emu: u64,
    theme: PresenterTheme,
) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("Cannot write {path:?}: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let n = slides.len();
    let deck_label = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Ének");

    let mut add = |name: &str, body: &str| -> Result<(), String> {
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zip, body.as_bytes()).map_err(|e| e.to_string())
    };

    let overrides = (1..=n)
        .map(|i| format!(r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#))
        .collect::<String>();
    add(
        "[Content_Types].xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>{overrides}</Types>"#
        ),
    )?;

    add(
        "_rels/.rels",
        &format!(
            r#"{REL_NS}<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
        ),
    )?;

    let sld_ids = (1..=n)
        .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + i, i + 1))
        .collect::<String>();
    add(
        "ppt/presentation.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx="{width_emu}" cy="{height_emu}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
        ),
    )?;

    let slide_rels = (1..=n)
        .map(|i| format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{i}.xml"/>"#, i + 1))
        .collect::<String>();
    add(
        "ppt/_rels/presentation.xml.rels",
        &format!(
            r#"{REL_NS}<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>{slide_rels}<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/></Relationships>"#,
            n + 2
        ),
    )?;

    add(
        "ppt/slideMasters/slideMaster1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {NS}><p:cSld><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
        ),
    )?;
    add(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        &format!(
            r#"{REL_NS}<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#
        ),
    )?;

    add(
        "ppt/slideLayouts/slideLayout1.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        ),
    )?;
    add(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        &format!(
            r#"{REL_NS}<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#
        ),
    )?;

    add("ppt/theme/theme1.xml", THEME_XML)?;

    for (i, slide) in slides.iter().enumerate() {
        let xml = match theme {
            PresenterTheme::Classic => classic_slide_xml(slide, width_emu, height_emu),
            PresenterTheme::Editorial => {
                editorial_slide_xml(slide, deck_label, width_emu, height_emu)
            }
        };
        add(&format!("ppt/slides/slide{}.xml", i + 1), &xml)?;
        add(
            &format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            &format!(
                r#"{REL_NS}<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#
            ),
        )?;
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}
