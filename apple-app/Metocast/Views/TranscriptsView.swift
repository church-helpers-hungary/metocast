import SwiftUI
import UniformTypeIdentifiers
import Metocast

/// Every stored English transcript: read one, download it, have AI clean it up, or delete it.
struct TranscriptsView: View {
    let session: MetocastSession
    @State private var transcripts: [StoredTranscriptRecord] = []
    @State private var failure: String?

    var body: some View {
        List {
            ForEach(transcripts, id: \.id) { transcript in
                NavigationLink {
                    TranscriptDetailView(session: session, id: transcript.id) { Task { await load() } }
                        .windowActions()
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(transcript.eventTitle ?? String(localized: "No event"))
                        Text(transcript.summary)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            }
        }
        .overlay {
            if transcripts.isEmpty && failure == nil {
                ContentUnavailableView(
                    "No Transcripts",
                    systemImage: "captions.bubble",
                    description: Text("Each live English transcript is kept here from the moment it starts.")
                )
            }
        }
        .navigationTitle("Transcripts")
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        do {
            transcripts = try await session.transcripts()
            failure = nil
        } catch {
            failure = error.metocastMessage
        }
    }
}

private struct TranscriptDetailView: View {
    let session: MetocastSession
    let id: String
    let changed: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var detail: TranscriptDetailRecord?
    @State private var clean = false
    @State private var failure: String?
    @State private var confirmingDelete = false
    @State private var export: TranscriptFile?

    var body: some View {
        List {
            if let detail {
                Section {
                    Text(detail.transcript.summary).foregroundStyle(.secondary)
                    if detail.transcript.cleanupStatus == "succeeded" {
                        Picker("Version", selection: $clean) {
                            Text("Original").tag(false)
                            Text("Cleaned Up").tag(true)
                        }
                        .pickerStyle(.segmented)
                    }
                    if let cleanup = cleanupLine(detail.transcript) {
                        Text(cleanup).foregroundStyle(detail.transcript.cleanupStatus == "failed" ? .red : .secondary)
                    }
                    if let captions = detail.transcript.captionsLine {
                        Text(captions).foregroundStyle(detail.transcript.captionsFailed > 0 ? .orange : .secondary)
                    }
                    if detail.transcript.captionsFailed > 0, let error = detail.transcript.captionLastError {
                        Text("Last caption error: \(error)").foregroundStyle(.red).textSelection(.enabled)
                    }
                }
                Section {
                    ForEach(lines(detail), id: \.seq) { line in
                        Text(clean ? line.cleanText ?? line.text : line.text)
                            .textSelection(.enabled)
                    }
                }
            }
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            }
        }
        .navigationTitle(detail?.transcript.eventTitle ?? String(localized: "Transcript"))
        .toolbar {
            if let transcript = detail?.transcript {
                Menu {
                    ForEach(["txt", "srt", "vtt"], id: \.self) { format in
                        Button(format.uppercased()) { Task { await download(format) } }
                    }
                } label: {
                    Label("Download", systemImage: "square.and.arrow.down")
                }
                if !transcript.live {
                    Button {
                        Task { await cleanUp() }
                    } label: {
                        Label("Clean Up with AI", systemImage: "wand.and.stars")
                    }
                    .disabled(transcript.cleanupStatus == "running")
                    Button(role: .destructive) {
                        confirmingDelete = true
                    } label: {
                        Label("Delete", systemImage: "trash")
                    }
                    .disabled(transcript.cleanupStatus == "running")
                }
            }
        }
        .confirmationDialog("Delete this transcript?", isPresented: $confirmingDelete) {
            Button("Delete", role: .destructive) { Task { await delete() } }
        } message: {
            Text("The original and the cleaned-up text are both removed. This cannot be undone.")
        }
        .fileExporter(
            isPresented: Binding(get: { export != nil }, set: { if !$0 { export = nil } }),
            document: export,
            contentType: export?.type ?? .plainText,
            defaultFilename: export?.name
        ) { result in
            if case .failure(let error) = result { failure = error.localizedDescription }
        }
        .task { await load() }
        .onChange(of: session.transcriptCleanup) { _, step in
            if let step, step.sessionId == id, step.state != "running" {
                Task { await load() }
                changed()
            }
        }
    }

    /// The lines of the version shown; the clean-up's empty lines are filler it dropped.
    private func lines(_ detail: TranscriptDetailRecord) -> [TranscriptLineRecord] {
        clean ? detail.lines.filter { ($0.cleanText ?? $0.text).isEmpty == false } : detail.lines
    }

    private func cleanupLine(_ transcript: StoredTranscriptRecord) -> String? {
        switch transcript.cleanupStatus {
        case "running":
            let step = session.transcriptCleanup?.sessionId == id ? session.transcriptCleanup?.progress ?? 0 : 0
            return String(localized: "Cleaning up with AI… \(step.formatted(.percent.precision(.fractionLength(0))))")
        case "failed": return String(localized: "Clean-up failed: \(transcript.cleanupError ?? String(localized: "unknown error"))")
        case "none" where !transcript.live:
            return String(localized: "Clean Up with AI has OpenAI rewrite each line into clearer English and keeps the original.")
        default: return nil
        }
    }

    private func load() async {
        do {
            let loaded = try await session.transcript(id: id)
            if detail == nil { clean = loaded.transcript.cleanupStatus == "succeeded" }
            detail = loaded
            failure = nil
        } catch {
            failure = error.metocastMessage
        }
    }

    private func download(_ format: String) async {
        guard let transcript = detail?.transcript else { return }
        do {
            let text = try await session.downloadTranscript(id: id, format: format, clean: clean)
            let date = String(transcript.startedAt.prefix(16)).replacingOccurrences(of: ":", with: "")
            export = TranscriptFile(
                text: text,
                // English, as the library is: the Media page reads `.en` from the name.
                name: "transcript-\(date)\(clean ? "-clean" : "").en",
                type: UTType(filenameExtension: format) ?? .plainText
            )
        } catch {
            failure = error.metocastMessage
        }
    }

    private func cleanUp() async {
        do {
            try await session.cleanupTranscript(id: id)
            await load()
        } catch {
            failure = error.metocastMessage
        }
    }

    private func delete() async {
        do {
            try await session.deleteTranscript(id: id)
            changed()
            dismiss()
        } catch {
            failure = error.metocastMessage
        }
    }
}

/// A downloaded transcript, handed to the system's save panel.
private struct TranscriptFile: FileDocument {
    static var readableContentTypes: [UTType] { [.plainText] }
    var text: String
    var name = "transcript"
    var type = UTType.plainText

    init(text: String, name: String, type: UTType) {
        self.text = text
        self.name = name
        self.type = type
    }

    init(configuration: ReadConfiguration) {
        text = String(decoding: configuration.file.regularFileContents ?? Data(), as: UTF8.self)
    }

    func fileWrapper(configuration: WriteConfiguration) -> FileWrapper {
        FileWrapper(regularFileWithContents: Data(text.utf8))
    }
}

private extension StoredTranscriptRecord {
    /// Date, length, lines, and whether it is live or cleaned up.
    var summary: String {
        let date = Date(rfc3339: startedAt).map { $0.formatted(date: .abbreviated, time: .shortened) } ?? startedAt
        let minutes = Int((durationSeconds / 60).rounded())
        var parts = [date, String(localized: "\(minutes) min"), String(localized: "\(segmentCount) lines")]
        if live { parts.append(String(localized: "live now")) }
        if cleanupStatus == "succeeded" { parts.append(String(localized: "cleaned up")) }
        return parts.joined(separator: " · ")
    }

    /// How many live YouTube captions went out, and to which stream; nil when none were sent.
    var captionsLine: String? {
        guard captionsDelivered + captionsFailed > 0 else { return nil }
        let counts = String(localized: "YouTube captions: \(captionsDelivered) delivered, \(captionsFailed) failed")
        guard let captionStreamId else { return counts }
        return counts + " · " + String(localized: "stream \(captionStreamId)")
    }
}
