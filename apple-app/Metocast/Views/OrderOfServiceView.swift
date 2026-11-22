import SwiftUI
import Metocast

extension OrderItemRecord {
    var icon: String {
        switch kind {
        case .song: "music.note"
        case .bible: "book"
        case .block: "text.alignleft"
        case .announcements: "megaphone"
        case .text: "text.below.photo"
        }
    }

    /// Whether the notes are what the step shows on screen (bullets or paragraphs), not notes
    /// for the people leading.
    var notesAreSlides: Bool { kind == .announcements || kind == .text }

    /// The announcements' bullet points, or a text slide's paragraphs: the notes' non-empty lines.
    var bullets: [String] {
        notes.split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    /// What the step shows, under its title: the deck, or which reading.
    var detail: String? {
        switch kind {
        case .song: deckPath.map { URL(filePath: $0).deletingPathExtension().lastPathComponent }
        case .bible: bibleType == nil ? reference : bibleType == "textus" ? "Textus" : "Lekció"
        case .block: nil
        case .announcements: bullets.isEmpty ? String(localized: "No announcements yet") : String(localized: "\(bullets.count) announcements")
        case .text: String(localized: "Slide")
        }
    }
}

/// One step as every list shows it. The step on screen is marked, so the person running the
/// service always sees where they are.
struct OrderStepRow: View {
    let item: OrderItemRecord
    let number: Int
    var isCurrent = false
    var isLoading = false
    /// A reading step's reference, e.g. "Jn 3,16-21", when the row knows the event's readings.
    var reading: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text("\(number)")
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(minWidth: 20, alignment: .trailing)
            Image(systemName: item.icon)
                .foregroundStyle(isCurrent ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title.isEmpty ? (item.detail ?? String(localized: "Untitled")) : item.title)
                    .fontWeight(isCurrent ? .semibold : .regular)
                if let detail = subtitle {
                    Text(detail).font(.subheadline).foregroundStyle(.secondary)
                }
                // The announcements' and a slide's notes are what they show; the row counts the bullets.
                if !item.notes.isEmpty, !item.notesAreSlides {
                    Text(item.notes).font(.subheadline).foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
            if isLoading {
                ProgressView()
                    .controlSize(.small)
                    .padding(4)
                    .glassEffect(.regular, in: .capsule)
            } else if isCurrent {
                Image(systemName: "play.fill")
                    .foregroundStyle(.tint)
                    .accessibilityHidden(true)
            }
        }
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
        .accessibilityValue(isLoading ? "Opening" : (isCurrent ? "On screen" : ""))
    }

    private var subtitle: String? {
        if item.kind == .bible, let reading { return reading }
        guard let detail = item.detail, !item.title.isEmpty, detail != item.title else { return nil }
        return detail
    }
}

/// The Order of Service on an event's page, edited in place: drag to reorder, swipe to delete,
/// double-click (or the pencil on the Mac) to rename, and the last row adds a step. Every change
/// is saved at once; steps keep their ids, so editing during the service is safe.
struct EventOrderSection: View {
    let session: MetocastSession
    let event: EventRecord
    @State private var steps: [OrderStep] = []
    @State private var renaming: String?
    @State private var newTitle = ""
    @State private var deckTarget: DeckTarget?
    @State private var failure: String?
    @State private var saves = 0
    /// The row a dragged step would land on, for the drop line.
    @State private var dropTarget: String?

    var body: some View {
        Section {
            ForEach($steps) { $step in
                row($step)
            }
            .onMove { from, to in
                steps.move(fromOffsets: from, toOffset: to)
                save()
            }
            HStack {
                TextField("Add a step", text: $newTitle)
                    .onSubmit { add(.block) }
                Menu {
                    Button("Song…", systemImage: "music.note") { deckTarget = DeckTarget(stepId: nil) }
                    Button("Announcements", systemImage: "megaphone") { add(.announcements) }
                    Button("Slide", systemImage: "text.below.photo") { add(.text) }
                    Button("Bible…", systemImage: "book") { add(.bible) }
                    Button("Block", systemImage: "text.alignleft") { add(.block) }
                } label: {
                    #if os(macOS)
                    Label("Add", systemImage: "plus")
                    #else
                    Image(systemName: "plus.circle.fill")
                        .font(.title3)
                        .foregroundStyle(.tint)
                    #endif
                }
                #if os(macOS)
                .buttonStyle(.glass)
                .labelStyle(.titleAndIcon)
                #else
                .buttonStyle(.borderless)
                #endif
                .fixedSize()
            }
        } header: {
            Text("Order of Service")
        } footer: {
            Text("Drag to reorder, swipe left (on the Mac, the trash button) to delete, double-click to rename. Announcements take one bullet per line and a slide one paragraph per line; both show on screen. The readings join the list when they are set on the event. Step through it on the Live or Presentation page.")
        }
        .sheet(item: $deckTarget) { target in
            NavigationStack {
                SongPickerView(session: session) { file in pick(file, for: target.stepId) }
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { deckTarget = nil }
                        }
                    }
            }
            #if os(macOS)
            .frame(minWidth: 420, minHeight: 480)
            #endif
        }
        .alert("Couldn't Save", isPresented: Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(failure ?? "")
        }
        .task(id: session.orderRevision) {
            // A change from elsewhere mustn't throw away a rename being typed.
            guard renaming == nil else { return }
            if let items = try? await session.order(eventId: event.id) {
                steps = items.map(OrderStep.init)
            }
        }
    }

    private func row(_ step: Binding<OrderStep>) -> some View {
        let id = step.wrappedValue.id
        let item = step.wrappedValue.item
        return HStack(spacing: 8) {
            if renaming == id {
                // Only this row's rename: its fields also finish as they disappear, after another row took over.
                StepFields(session: session, item: step.item) { if renaming == id { finishRename() } }
            } else {
                OrderStepRow(
                    item: item,
                    number: (steps.firstIndex { $0.id == id } ?? 0) + 1,
                    isCurrent: session.orderPosition?.eventId == event.id && session.orderPosition?.itemId == item.id,
                    reading: item.kind != .bible ? nil
                        : item.bibleType == nil ? item.reference
                        : event.bibleReferences.first { $0.referenceType == item.bibleType }?.reference ?? String(localized: "Not set on the event yet")
                )
                .onTapGesture(count: 2) { rename(id) }
            }
            if item.kind == .song {
                Button(item.deckPath == nil ? "Choose Deck" : "Change Deck", systemImage: "rectangle.stack") {
                    deckTarget = DeckTarget(stepId: id)
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
            }
            #if os(macOS)
            // Swipe isn't a Mac gesture, and `.draggable` takes the right-click menu: the row's own buttons.
            Button {
                renaming == id ? finishRename() : rename(id)
            } label: {
                Label {
                    Text("Rename")
                } icon: {
                    SwapSymbol(renaming == id ? "checkmark" : "pencil", among: ["checkmark", "pencil"])
                }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            Button("Delete", systemImage: "trash", role: .destructive) { delete(id) }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.red)
            #endif
        }
        #if os(macOS)
        // A grouped Form on the Mac ignores `.onMove`, so rows drag and drop themselves.
        .draggable(id)
        .dropDestination(for: String.self) { ids, _ in
            guard let dragged = ids.first else { return false }
            return move(dragged, onto: id)
        } isTargeted: { targeted in
            dropTarget = targeted ? id : (dropTarget == id ? nil : dropTarget)
        }
        .overlay(alignment: .top) {
            if dropTarget == id {
                Rectangle().fill(.tint).frame(height: 2).offset(y: -6)
            }
        }
        #endif
        .swipeActions(edge: .trailing) {
            Button("Delete", systemImage: "trash", role: .destructive) { delete(id) }
        }
        .contextMenu {
            Button("Rename", systemImage: "pencil") { rename(id) }
            Button("Delete", systemImage: "trash", role: .destructive) { delete(id) }
        }
    }

    private func rename(_ id: String) {
        if renaming != nil { save() }
        renaming = id
    }

    private func finishRename() {
        guard renaming != nil else { return }
        renaming = nil
        save()
    }

    /// Puts the dragged step where the target row is: above it when moving up, below when down.
    private func move(_ dragged: String, onto target: String) -> Bool {
        guard let from = steps.firstIndex(where: { $0.id == dragged }),
              let to = steps.firstIndex(where: { $0.id == target }), from != to else { return false }
        steps.move(fromOffsets: [from], toOffset: to > from ? to + 1 : to)
        dropTarget = nil
        save()
        return true
    }

    private func delete(_ id: String) {
        steps.removeAll { $0.id == id }
        save()
    }

    /// Adds a step named by the text field. A block needs a name; announcements, a slide and a
    /// Bible reading get one if it's empty and open for editing, since what they show is typed in them.
    private func add(_ kind: OrderItemKind) {
        var title = newTitle.trimmingCharacters(in: .whitespaces)
        if title.isEmpty {
            switch kind {
            case .announcements: title = String(localized: "Announcements")
            case .text: title = String(localized: "Welcome")
            case .bible: title = String(localized: "Bible")
            default: return
            }
        }
        let step = OrderStep(OrderItemRecord(id: nil, kind: kind, title: title, deckPath: nil, bibleType: nil, reference: nil, translation: nil, notes: ""))
        steps.append(step)
        newTitle = ""
        if kind == .block { save() } else { rename(step.id) }
    }

    /// A song placeholder takes the deck and its name; with no step, the deck is a new song.
    private func pick(_ file: PptFile, for stepId: String?) {
        // A browser download can leave a name percent-encoded ("P%C3%A9csi…").
        let name = (file.name.removingPercentEncoding ?? file.name).replacing(/\.pptx?$/, with: "")
        if let index = steps.firstIndex(where: { $0.id == stepId }) {
            steps[index].item.deckPath = file.path
            steps[index].item.title = name
        } else {
            steps.append(OrderStep(OrderItemRecord(id: nil, kind: .song, title: name, deckPath: file.path, bibleType: nil, reference: nil, translation: nil, notes: "")))
        }
        save()
    }

    /// Sends the whole list. Only the latest save's answer is kept, so a slow reply can't undo a
    /// newer edit; a refused list is reloaded from the server.
    private func save() {
        saves += 1
        let attempt = saves
        let items = steps.map(\.item)
        Task {
            do {
                let stored = try await session.saveOrder(eventId: event.id, items: items)
                if attempt == saves, renaming == nil { steps = stored.map(OrderStep.init) }
            } catch {
                failure = error.metocastMessage
                if let items = try? await session.order(eventId: event.id) { steps = items.map(OrderStep.init) }
            }
        }
    }
}

/// A step in the list, keyed by its server id, or by a local one until the server gives it one.
private struct OrderStep: Identifiable {
    let id: String
    var item: OrderItemRecord

    init(_ item: OrderItemRecord) {
        id = item.id ?? UUID().uuidString
        self.item = item
    }
}

/// Which song the deck picker is choosing for; none adds a new song.
private struct DeckTarget: Identifiable {
    let stepId: String?
    var id: String { stepId ?? "" }
}

/// A step's title and notes typed in place, and for a part without a deck what it shows: nothing,
/// a slide (one paragraph per line) or the announcements (one bullet per line). Return in the
/// title, Done, or leaving the page finishes; focus moving to the picker doesn't.
private struct StepFields: View {
    let session: MetocastSession
    @Binding var item: OrderItemRecord
    let done: () -> Void
    @FocusState private var focus: Field?
    /// What looking the reference up found: nil before the first try.
    @State private var lookup: Result<Int, Error>?
    @State private var isLookingUp = false

    private enum Field { case title, notes }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            TextField("Title", text: $item.title)
                .fontWeight(.medium)
                .focused($focus, equals: .title)
                .onSubmit(done)
            // A block made before slides existed (the old welcome) can become one here.
            if [.block, .text, .announcements].contains(item.kind) {
                Picker("Shows", selection: $item.kind) {
                    Text("Nothing").tag(OrderItemKind.block)
                    Text("Slide").tag(OrderItemKind.text)
                    Text("Announcements").tag(OrderItemKind.announcements)
                }
                .pickerStyle(.segmented)
            }
            // A reading of its own; the event's Lekció and Textus are set on the event instead.
            if item.kind == .bible, item.bibleType == nil {
                TextField("Reference", text: Binding(get: { item.reference ?? "" }, set: { item.reference = $0 }), prompt: Text("Zsolt 23"))
                    .autocorrectionDisabled()
                BibleTranslationPicker(
                    label: String(localized: "Bible"),
                    selection: Binding(get: { item.translation ?? MetocastSession.defaultBibleTranslation }, set: { item.translation = $0 })
                )
                Group {
                    if isLookingUp {
                        Label("Looking up…", systemImage: "magnifyingglass")
                    } else if case .success(let count) = lookup, count == 0 {
                        Label("No verses found", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    } else if case .success(let count) = lookup {
                        Label("^[\(count) verse](inflect: true) found", systemImage: "checkmark.circle")
                            .foregroundStyle(.green)
                    } else if case .failure(let error) = lookup {
                        Label(error.metocastMessage, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }
                .font(.footnote)
                // Like the event editor: once typing pauses; a newer edit cancels this one.
                .task(id: "\(item.reference ?? "")|\(item.translation ?? "")") { await lookUp() }
            }
            if item.notesAreSlides {
                TextEditor(text: $item.notes)
                    .frame(minHeight: 100)
                    .focused($focus, equals: .notes)
                    .overlay(alignment: .topLeading) {
                        if item.notes.isEmpty {
                            Group {
                                if item.kind == .announcements {
                                    Text("One announcement per line")
                                } else {
                                    Text("Optional paragraphs, one per line")
                                }
                            }
                            .foregroundStyle(.tertiary)
                            .padding(.leading, 5)
                            .allowsHitTesting(false)
                        }
                    }
            } else {
                TextField("Notes for the leaders", text: $item.notes, axis: .vertical)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .focused($focus, equals: .notes)
            }
            Button("Done", action: done)
                .buttonStyle(.glassProminent)
        }
        .onAppear { focus = .title }
        .onDisappear(perform: done)
    }

    /// Checks the reference the server will look up on save, so a typo shows before Done.
    private func lookUp() async {
        let reference = (item.reference ?? "").trimmingCharacters(in: .whitespaces)
        guard !reference.isEmpty else {
            lookup = nil
            return
        }
        do {
            try await Task.sleep(for: .milliseconds(500))
        } catch {
            return
        }
        isLookingUp = true
        defer { isLookingUp = false }
        do {
            let verses = try await session.verses(for: reference, translation: item.translation ?? MetocastSession.defaultBibleTranslation)
            lookup = .success(verses.count)
        } catch {
            if !Task.isCancelled { lookup = .failure(error) }
        }
    }
}

/// The deck library, as a pushed page that picks one deck for a song step.
struct SongPickerView: View {
    let session: MetocastSession
    let onPick: (PptFile) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""

    var body: some View {
        DeckList(session: session, query: query) { file in
            onPick(file)
            dismiss()
        }
        .librarySearch($query)
        .navigationTitle("Choose Song")
    }
}

/// The Order of Service of the event under way, on the Live page: every step, the one on screen
/// marked, and a click on any step shows it.
struct LiveOrderSection: View {
    let session: MetocastSession
    let event: EventSummaryRecord
    @State private var items: [OrderItemRecord] = []
    @State private var isLoaded = false

    var body: some View {
        Section {
            // A row either way: an empty section never appears, so its task would never load.
            if !isLoaded {
                ProgressView()
                    .frame(maxWidth: .infinity)
            } else if items.isEmpty {
                Text("\(event.title) has no Order of Service yet. Add one on the event's page.")
                    .foregroundStyle(.secondary)
            }
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                Button {
                    if let id = item.id { session.show(.orderGo(eventId: event.id, itemId: id)) }
                } label: {
                    OrderStepRow(
                        item: item,
                        number: index + 1,
                        isCurrent: isCurrent(item),
                        isLoading: item.id != nil && session.loadingOrderItemId == item.id
                    )
                }
                .buttonStyle(.plain)
                .accessibilityHint("Shows this step on the output screen")
            }
            if !items.isEmpty {
                GlassEffectContainer(spacing: 12) {
                    HStack(spacing: 12) {
                        Button("Previous Step", systemImage: "chevron.backward") {
                            session.show(.orderPrevious(eventId: event.id))
                        }
                        // The arrow alone, so both buttons stay on one line in the narrow sidebar.
                        .labelStyle(.iconOnly)
                        .buttonStyle(.glass)
                        .disabled(session.isSteppingOrder || (currentIndex.map { $0 == 0 } ?? true))
                        Spacer()
                        if session.isSteppingOrder {
                            ProgressView()
                                .controlSize(.small)
                        }
                        Button("Next Step", systemImage: "chevron.forward") {
                            session.show(.orderNext(eventId: event.id))
                        }
                        .lineLimit(1)
                        .buttonStyle(.glassProminent)
                        .foregroundStyle(.white)
                        .disabled(session.isSteppingOrder || (currentIndex.map { $0 >= items.count - 1 } ?? false))
                    }
                }
            }
        }
        .task(id: "\(event.id)-\(session.orderRevision)") {
            items = (try? await session.order(eventId: event.id)) ?? items
            isLoaded = true
        }
    }

    private var currentIndex: Int? {
        guard session.orderPosition?.eventId == event.id else { return nil }
        return items.firstIndex { $0.id == session.orderPosition?.itemId }
    }

    private func isCurrent(_ item: OrderItemRecord) -> Bool {
        session.orderPosition?.eventId == event.id && session.orderPosition?.itemId == item.id
    }
}
