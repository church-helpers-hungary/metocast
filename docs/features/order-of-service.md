# Order of Service and the slide library

An event's **Order of Service** is the service's steps in order, the way the printed sheet lists
them. Each step is one of five kinds:

- **Song**: a deck from the slide library. The hymn number goes in the title and the verses in the
  notes, e.g. _Ó, hála az Úrnak (47)_ with _2. vers – közben kollekta_. A song can also have no
  deck, such as a closing verse sung without slides.
- **Bible reading**: the event's own _Lekció_ or _Textus_, or a passage of its own (**Add →
  Bible…**, then type the reference and pick the translation). The verses are stored, on the event
  or with the step when the list is saved, so showing them needs no network. A reference that
  finds no verses is refused when you save.
- **Announcements**: _Hirdetések_. You type the announcements into the step, one per
  line, and the server turns them into slides: the title, then the bullet points, five to a slide.
  A long bullet is broken onto several lines of at most 34 characters, at a comma or other
  punctuation where one fits, or else between words, so it reads at full size instead of
  shrinking the slide; it still counts as one of the five.
  Like a song, they open in the web presenter, or, in Keynote mode, as `hirdetesek.pptx` in the
  Bible slides folder (a temporary folder when none is set). An announcements step with no
  bullets yet shows its title alone.
- **Slide**: one slide of its own, such as the welcome: its title, and under it optional
  paragraphs, one per line, typed into the step. Long paragraphs are broken onto lines the same
  way as long announcements. With the web presenter it shows as the
  [welcome screen](welcome-screen.md) (photo, logo, readable text); in Keynote mode it opens as a
  plain `szoveg.pptx`.
- **Block**: a part with no slides, such as _Köszöntés, imádság_ or _Igehirdetés_ (the speaker goes
  in the notes).

During the service the operator steps through the list:

- a song opens its deck in the web presenter or Keynote;
- a reading loads the event's verses, bookended like a song: a title slide with the reading's name
  and reference (_Lekció — Jn 3:16-17_), then the verse pages, then a blank closing slide. The
  counter on each verse page (_1/3_) counts verse pages only. A verse page holds at most 35 words
  (whole verses where they fit; a longer verse is split across pages), so the text stays large
  enough to read from the back;
- the announcements show their bullet points as slides, and a slide step its slide;
- a block leaves the screen as it is.

Every connected device sees the same list and the same current step.

## A new event's steps

Every new event starts with these steps, whichever app or tool created it:

1. Isten hozott. (a slide, named after the [welcome screen](welcome-screen.md)'s greeting)
2. Song 1
3. Song 2
4. Lekció
5. Song 3
6. Textus
7. Preaching
8. Song 4
9. Hirdetések
10. Áldás
11. Song 4 last part

The songs are placeholders with no deck: pick one from the slide library and the step takes the
deck and its name. Lekció and Textus are the event's readings, and each shows its reference once it
is set on the event. The first step is the welcome slide: double-tap it to change its title and add
paragraphs. _Hirdetések_ is an announcements step: double-tap it and type this week's
announcements, one per line. The server adds the steps when it creates the event, so Presentation, Companion's
_Next Step_ and every other client can step through a new event before anyone edits it.

**The readings join by themselves.** When a Lekció or Textus is set or changed on an event whose
list has no step for it, the server adds one: the Lekció just before the Textus, the Textus just
after the Lekció, otherwise at the end. A reading saved again unchanged adds nothing, so a reading
step you deleted stays deleted until the reading itself changes. Clearing a reading leaves its
step in the list.

## Use it

The list is edited on the event's page, where it is shown. There is no separate editor and no Save
button: every change is saved at once.

**Apple app**

- **Events → an event → Lekció / Textus**: type the reference or pick another translation. Once
  you stop typing, the verses are looked up and the event is saved; the Order of Service's reading step
  follows (and is added if the list has none). Empty the reference to remove the reading.
- **Events → an event → Order of Service**:
  - drag a step to move it;
  - swipe left to delete it on the iPhone; on the Mac click its trash button;
  - double-tap a step to rename it and edit its notes, or click its pencil on the Mac. Return in
    the title or **Done** finishes. On the announcements the box takes one bullet per line, on a
    slide one paragraph per line. A part without a deck has **Shows**: _Nothing_ (a block),
    _Slide_ or _Announcements_, so a block from an older event, such as its welcome, can become a
    slide;
  - the deck button on a song picks or changes its deck from the library;
  - the last row adds a step: type its name and press Return for a block, or use **Add** for a
    **Song…** from the library, **Announcements**, a **Slide**, a **Bible…** reading or
    a **Block**.
- **Live** and the Dashboard's Presentation card have a button that opens **Presentation** for the
  current event.
- **Presentation** is where the service is run, and holds the slide library:
  - every deck in the watched folders, with search by title, hymn number or lyrics and a
    folder filter (see [Slide library search](slide-library.md));
  - one click opens a deck;
  - the folder menu adds and removes watched folders.
  - The sidebar (on the iPhone, the top of the page) switches between
    **Order of Service** and **Library**. Order of Service is the list of the event on screen, or
    else the one under way or next. Click a step to show it, or use **Previous Step** / **Next
    Step**, to step through the service while watching the slides.

**Sanctum**

- **Events**: the event's page has the same list under the event form, edited in place: drag a
  step by its handle (or use the arrows), swipe left to delete, double-click or the pencil to
  rename, the deck button on a song, and _Add a step_ at the end (with _Add song_, _Add
  announcements_, _Add slide_ and _Add Bible reading_). While renaming, _Shows_ switches a part
  without a deck between nothing, a slide and the announcements. Every change is saved at once.
  The readings are edited in the event form above it.
- **Home** runs the current event's Order of Service.
- **Presentations** has the full library under the quick search.

**Shared view**

`/ui/sanctum/presenter?view=order` shows the running Order of Service with the current step
highlighted. It is read-only, needs no login, and suits a stage screen or the people following
along.

**Companion**

- Actions: _Order of Service: Next Step_ and _Previous Step_.
- Feedback: _Step On Screen_.
- Variables: `$(metocast:order_step)`, `$(metocast:order_step_number)`, e.g. `3/10`, and
  `$(metocast:order_next_step)`, the step after the one on screen.
- With nothing running, Next Step starts the presenter's selected event at step 1.

## Entry points

| Does                              | REST                         | WebSocket                      | MCP tool                       |
| --------------------------------- | ---------------------------- | ------------------------------ | ------------------------------ |
| Create an event with the template | `POST /api/events`           | `events.create`                | `events_create`                |
| Add a reading's step              | `PUT /api/events/{id}`       | `events.update`                | `events_update`                |
| Read an event's steps             | `GET /api/events/{id}/order` |                                | `events_order_get`             |
| Replace an event's steps          | `PUT /api/events/{id}/order` |                                | `events_order_set`             |
| Show a step                       |                              | `order.go`                     | `order_go`                     |
| Next / previous step              |                              | `order.next`, `order.previous` | `order_next`, `order_previous` |
| Which step is on screen           |                              | `order.status`                 | `order_status`                 |

Pushes:

- `order.state` carries the step on screen and that event's steps. It is sent on connect and on
  every change.
- `event.order.changed` is sent when an event's list is replaced, and when an event update adds a
  reading's step.

Notes:

- `PUT` replaces the whole list. Send each step's `id` back to keep it, so the step on screen
  stays current while you edit during the service. The apps send the whole list after every edit.
- When the edited event is on screen, `PUT` also re-sends `order.state`, so a step renamed during
  the service shows at once on every device and in Companion.
- The live position is kept in memory, so a server restart starts with no step on screen.
- `order.status` is allowed without the token, like `presenter.status`, so the shared view works
  without logging in.
- The web presenter sends a whole deck, pictures included, to every device as one message of at
  most 64 MB. A larger deck is refused: the screen keeps showing the current deck, and the device
  that opened it gets an error saying how large the deck is. Every picture is embedded again on
  each slide that uses it, so the server first scales each picture down to the slide's size
  (1280 pixels on the long side) and stores opaque ones as JPEG: a 15 MB photo shown on four
  slides comes to under half a megabyte. Only decks with very many distinct pictures still
  reach the limit; shrink those in PowerPoint with _Picture Format → Compress Pictures_.

The slide library uses `GET /api/ppt/files`, `GET/POST /api/ppt/folders` and
`DELETE /api/ppt/folders/{id}`; [Slide library search](slide-library.md) describes the search
and its index.
