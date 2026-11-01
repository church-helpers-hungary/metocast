import SwiftUI
import UniformTypeIdentifiers
import Metocast
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// Settings that matter only while presenting, a page inside Presentation rather than kept in
/// Settings: how decks are shown, the welcome screen, the Raspberry Pi receiver, and on a Mac the
/// slide fonts.
struct PresentationSettingsView: View {
    @Environment(AppModel.self) private var model
    let session: MetocastSession

    var body: some View {
            Form {
                Section {
                    Toggle("Web Presenter", isOn: Binding(
                        get: { session.usesWebPresenter },
                        set: { session.show(.setUseWebPresenter(enabled: $0)) }
                    ))
                    Picker("Slide Design", selection: Binding(
                        get: { session.presenterTheme },
                        set: { session.show(.setPresenterTheme(theme: $0)) }
                    )) {
                        Text("Classic").tag(PresenterTheme.classic)
                        Text("Editorial").tag(PresenterTheme.editorial)
                    }
                } header: {
                    Text("Presentation")
                } footer: {
                    Text(session.usesWebPresenter
                        ? "Decks open in Metocast's own presenter on the server's screen."
                        : "Decks open in Keynote on the server's Mac.")
                }
                Section {
                    CopyableLinks(urls: presenterURLs)
                } header: {
                    Text("Web Presenter Link")
                } footer: {
                    Text("Open it in a browser on any device on this network to show the slides. The IP address works everywhere; the .local name needs Bonjour on that device.")
                }
                WelcomeScreenSection(session: session)
                ConnectedDevicesSection(session: session)
                PresenterInstallSection(baseURL: receiverBaseURL)
                #if os(macOS)
                PresentationFontsSection()
                #endif
            }
            .formStyle(.grouped)
            .navigationTitle("Presentation Settings")
    }

    /// Addresses other devices can reach, best first: the Mac hosting the server knows its
    /// network ones, while a client's own connection is already a network address.
    private var baseURLs: [String] {
        #if os(macOS)
        if model.mode == .server { return model.server.networkURLs }
        #endif
        return [session.baseURL]
    }

    private var receiverBaseURL: String { baseURLs[0] }

    private var presenterURLs: [String] {
        let token = session.token.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? session.token
        return baseURLs.map { "\($0.hasSuffix("/") ? String($0.dropLast()) : $0)/presenter?token=\(token)" }
    }
}

/// The welcome screen a Slide step shows: its text, how often the photo changes, the 12 photos
/// (each replaceable from this computer) and the logo. Text saves itself once typing pauses.
private struct WelcomeScreenSection: View {
    let session: MetocastSession
    @State private var settings: WelcomeSettingsRecord?
    /// Thumbnails by photo number; reloaded after a replace or reset.
    @State private var thumbnails: [UInt32: Data] = [:]
    /// What the file picker is replacing. It outlives the picker, whose binding is reset before
    /// its completion runs.
    @State private var importTarget: ImportTarget?
    @State private var importing = false
    @State private var busy: UInt32?
    @State private var failure: String?
    /// The month whose slide is being previewed.
    @State private var previewing: ImportTarget?
    /// A replace asked for in the preview, opened once the preview has closed.
    @State private var pendingImport: ImportTarget?
    /// The last text fields that were saved or loaded, so opening the page or resetting a photo
    /// doesn't trigger an unnecessary save.
    @State private var lastSavedText: String?

    /// The default greeting and blessing in each language, swapped in on a language change
    /// unless the operator wrote their own.
    private static let defaults = [
        "hu": ("Isten hozott.", "Az ÚR áldása legyen életeden!"),
        "en": ("Welcome.", "The Lord bless you and keep you!"),
    ]

    /// What the file picker is choosing for: a photo (1–12) or the logo (0).
    private struct ImportTarget: Identifiable {
        let number: UInt32
        var id: UInt32 { number }
    }

    private func textKey(for settings: WelcomeSettingsRecord) -> String {
        "\(settings.title)|\(settings.subtitle)|\(settings.churchName)|\(settings.language)"
    }

    var body: some View {
        Section {
            if let settings {
                TextField("Greeting", text: field(\.title), prompt: Text("Isten hozott."))
                TextField("Blessing", text: field(\.subtitle), prompt: Text("Az ÚR áldása legyen életeden!"))
                TextField("Church Name", text: field(\.churchName), prompt: Text("Nyíregyházi Metodista Gyülekezet"))
                Picker("Language", selection: Binding(get: { settings.language }, set: { switchLanguage(to: $0) })) {
                    Text(verbatim: "Magyar").tag("hu")
                    Text(verbatim: "English").tag("en")
                }
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 4), spacing: 12) {
                    ForEach(UInt32(1)...12, id: \.self) { number in
                        photo(number, replaced: settings.replacedPhotos.contains(number))
                    }
                }
                .padding(.vertical, 4)
                LabeledContent("Logo") {
                    GlassEffectContainer(spacing: 8) {
                        HStack(spacing: 8) {
                            // The church's cross-and-flame logo is built in; an upload replaces it.
                            Text(settings.hasLogo ? "Custom" : "Default").foregroundStyle(.secondary)
                            if settings.hasLogo {
                                Button("Reset to Default") { Task { await change { try await session.removeWelcomeLogo() } } }
                                    .buttonStyle(.glass)
                            }
                            Button("Replace…") { pick(ImportTarget(number: 0)) }
                                .buttonStyle(.glassProminent)
                        }
                    }
                }
            } else {
                ProgressView()
            }
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            }
        } header: {
            Text("Welcome Screen")
        } footer: {
            Text("A Slide step in the Order of Service shows this with the web presenter: the photo of the event's month with the logo, and its title, the blessing and the date in the lower third. Click a month to preview it, or right-click to replace or reset its photo. The text always stays readable: the photo softens and tones down only where text sits.")
        }
        .fileImporter(
            isPresented: $importing,
            allowedContentTypes: importTarget?.number == 0 ? [.png, .svg] : [.image]
        ) { result in
            guard let target = importTarget else { return }
            Task { await upload(result, to: target.number) }
        }
        .task { await load() }
        // Saved once typing pauses; a newer edit cancels the wait.
        .sheet(item: $previewing, onDismiss: {
            if let pendingImport { pick(pendingImport) }
            pendingImport = nil
        }) { target in
            WelcomePreview(
                session: session,
                month: target.number,
                isReplaced: settings?.replacedPhotos.contains(target.number) == true,
                replace: { pendingImport = target; previewing = nil },
                reset: { Task { await change(photo: target.number) { try await session.resetWelcomePhoto(target.number) } } }
            )
        }
        .task(id: settings.map(textKey(for:))) {
            guard let current = settings else { return }
            let key = textKey(for: current)
            guard key != lastSavedText else { return }
            do {
                try await Task.sleep(for: .milliseconds(800))
                // Not stored back: it would overwrite whatever was typed during the save.
                lastSavedText = textKey(for: try await session.saveWelcomeSettings(current))
                failure = nil
            } catch is CancellationError {
                return
            } catch {
                failure = error.metocastMessage
            }
        }
    }

    private func pick(_ target: ImportTarget) {
        importTarget = target
        importing = true
    }

    private func field(_ path: WritableKeyPath<WelcomeSettingsRecord, String>) -> Binding<String> {
        Binding(get: { settings?[keyPath: path] ?? "" }, set: { settings?[keyPath: path] = $0 })
    }

    /// A month's photo with the month's name; a click previews that month's slide.
    private func photo(_ number: UInt32, replaced: Bool) -> some View {
        Button { previewing = ImportTarget(number: number) } label: {
            VStack(spacing: 4) {
                ZStack {
                    Rectangle().fill(.quaternary)
                    if let data = thumbnails[number], let image = PlatformImage(data: data) {
                        Image(platformImage: image).resizable().scaledToFill()
                    }
                    if busy == number { ProgressView() }
                }
                .aspectRatio(16 / 9, contentMode: .fit)
                .clipShape(.rect(cornerRadius: 6))
                .overlay(alignment: .bottomTrailing) {
                    if replaced {
                        Image(systemName: "pencil.circle.fill").padding(4).foregroundStyle(.white).shadow(radius: 2)
                    }
                }
                Text(Calendar.current.standaloneMonthSymbols[Int(number) - 1].localizedCapitalized)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button("Preview", systemImage: "eye") { previewing = ImportTarget(number: number) }
            Button("Replace Photo…", systemImage: "photo") { pick(ImportTarget(number: number)) }
            if replaced {
                Divider()
                Button("Reset to Default", systemImage: "arrow.uturn.backward", role: .destructive) {
                    Task { await change(photo: number) { try await session.resetWelcomePhoto(number) } }
                }
            }
        }
        .accessibilityLabel(replaced
            ? Text("\(Calendar.current.standaloneMonthSymbols[Int(number) - 1]), photo replaced")
            : Text(Calendar.current.standaloneMonthSymbols[Int(number) - 1]))
        .accessibilityHint("Shows that month's welcome slide")
    }

    /// The other language's default text becomes this one's; text the operator wrote stays.
    private func switchLanguage(to language: String) {
        guard var updated = settings, let old = Self.defaults[updated.language], let new = Self.defaults[language] else { return }
        if updated.title == old.0 { updated.title = new.0 }
        if updated.subtitle == old.1 { updated.subtitle = new.1 }
        updated.language = language
        settings = updated
    }

    private func load() async {
        do {
            let loaded = try await session.welcomeSettings()
            lastSavedText = textKey(for: loaded)
            settings = loaded
        } catch {
            failure = error.metocastMessage
        }
        // One missing photo (not downloaded yet) leaves the other months' thumbnails.
        for number in UInt32(1)...12 {
            do {
                thumbnails[number] = try await session.welcomePhoto(number)
            } catch {
                failure = error.metocastMessage
            }
        }
    }

    /// Runs one upload, reset or removal, then re-reads what changed.
    private func change(photo number: UInt32? = nil, _ action: () async throws -> Void) async {
        busy = number
        defer { busy = nil }
        do {
            try await action()
            let saved = settings
            var reloaded = try await session.welcomeSettings()
            // Keep text typed since the last save.
            if let saved {
                reloaded.title = saved.title
                reloaded.subtitle = saved.subtitle
                reloaded.churchName = saved.churchName
                reloaded.language = saved.language
            } else {
                lastSavedText = textKey(for: reloaded)
            }
            settings = reloaded
            if let number { thumbnails[number] = try await session.welcomePhoto(number) }
            failure = nil
        } catch {
            failure = error.metocastMessage
        }
    }

    private func upload(_ result: Result<URL, Error>, to number: UInt32) async {
        do {
            let url = try result.get()
            let access = url.startAccessingSecurityScopedResource()
            defer { if access { url.stopAccessingSecurityScopedResource() } }
            let data = try Data(contentsOf: url)
            if number == 0 {
                await change { try await session.setWelcomeLogo(data) }
            } else {
                await change(photo: number) { try await session.replaceWelcomePhoto(number, with: data) }
            }
        } catch {
            failure = error.metocastMessage
        }
    }
}

/// One month's welcome slide as the screens will show it, with that photo's actions.
private struct WelcomePreview: View {
    @Environment(\.dismiss) private var dismiss
    let session: MetocastSession
    let month: UInt32
    let isReplaced: Bool
    let replace: () -> Void
    let reset: () -> Void
    @State private var svg: String?
    @State private var failure: String?

    var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                ZStack {
                    Color.black
                    if let svg {
                        SVGView(svg: SVGView.scalable(svg, width: 1280, height: 720))
                    } else if let failure {
                        Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.white)
                    } else {
                        ProgressView()
                    }
                }
                .aspectRatio(16 / 9, contentMode: .fit)
                .clipShape(.rect(cornerRadius: 10))
                GlassEffectContainer(spacing: 12) {
                    HStack(spacing: 12) {
                        if isReplaced {
                            Button("Reset to Default", systemImage: "arrow.uturn.backward") {
                                reset()
                                dismiss()
                            }
                            .buttonStyle(.glass)
                        }
                        Spacer()
                        Button("Replace…", systemImage: "photo") { replace() }
                            .buttonStyle(.glassProminent)
                    }
                }
            }
            .padding()
            .navigationTitle(Calendar.current.standaloneMonthSymbols[Int(month) - 1].localizedCapitalized)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 640, minHeight: 460)
        #endif
        .task {
            do {
                svg = try await session.welcomePreview(month)
            } catch {
                failure = error.metocastMessage
            }
        }
    }
}

/// Devices currently connected to the server over WebSocket (Raspberry Pi presenter receivers,
/// browser presenters, and other Metocast apps), with their live round-trip latency and Ping action.
private struct ConnectedDevicesSection: View {
    let session: MetocastSession
    @State private var pingingIds: Set<String> = []

    var body: some View {
        Section {
            if session.clients.isEmpty {
                Text("No other devices connected")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(session.clients, id: \.id) { client in
                    HStack(spacing: 12) {
                        Image(systemName: icon(for: client))
                            .font(.title3)
                            .foregroundStyle(.tint)
                            .frame(width: 24)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(client.label.isEmpty ? String(localized: "Unnamed device") : client.label)
                                .fontWeight(.medium)
                            if let subtitle = subtitle(for: client) {
                                Text(subtitle)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        Spacer(minLength: 8)
                        HStack(spacing: 10) {
                            if let latency = client.latencyMs {
                                Text("\(latency) ms")
                                    .font(.caption.monospacedDigit().weight(.medium))
                                    .foregroundStyle(latencyColor(latency))
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 3)
                                    .background(latencyColor(latency).opacity(0.14), in: .capsule)
                            } else {
                                Text("—")
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(.secondary)
                            }
                            Button {
                                ping(client.id)
                            } label: {
                                if pingingIds.contains(client.id) {
                                    ProgressView()
                                        .controlSize(.small)
                                } else {
                                    Label("Ping", systemImage: "wave.3.right")
                                        .font(.caption)
                                }
                            }
                            .buttonStyle(.glass)
                            .controlSize(.small)
                            .labelStyle(.iconOnly)
                            .disabled(pingingIds.contains(client.id))
                        }
                        // The device's name wraps instead; the button and latency keep their size.
                        .fixedSize()
                    }
                    .padding(.vertical, 2)
                    // Otherwise the row's divider starts at the button's label.
                    .alignmentGuide(.listRowSeparatorLeading) { _ in 0 }
                }
            }
        } header: {
            HStack {
                Text("Connected Devices (\(session.clients.count))")
                Spacer()
                if !session.clients.isEmpty {
                    Button("Ping All", systemImage: "wave.3.right") {
                        for client in session.clients {
                            ping(client.id)
                        }
                    }
                    .font(.caption)
                    .buttonStyle(.borderless)
                }
            }
        } footer: {
            Text("Devices connected to the server right now — Raspberry Pi receivers, web presenters, and other Metocast apps. Ping measures live WebSocket round-trip latency.")
        }
        .task {
            session.show(.clientsList)
        }
    }

    private func ping(_ clientId: String) {
        pingingIds.insert(clientId)
        session.pingClient(clientId)
        Task {
            try? await Task.sleep(for: .milliseconds(700))
            pingingIds.remove(clientId)
        }
    }

    private func icon(for client: ConnectedClientRecord) -> String {
        let label = client.label.lowercased()
        if label.contains("receiver") || label.contains("raspberry") || label.contains("pi") {
            return "play.display"
        }
        if label.contains("iphone") || label.contains("ios") {
            return "iphone"
        }
        if label.contains("ipad") {
            return "ipad"
        }
        if label.contains("mac") || label.contains("tauri") || label.contains("desktop") || label.contains("metocast") {
            return "laptopcomputer"
        }
        return "globe"
    }

    private func subtitle(for client: ConnectedClientRecord) -> String? {
        var parts: [String] = []
        if let hostname = client.hostname, !hostname.isEmpty {
            parts.append(hostname)
        }
        if let ua = client.userAgent, !ua.isEmpty {
            parts.append(shortUserAgent(ua))
        }
        if let duration = connectedDuration(client.connectedAt) {
            parts.append(duration)
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func shortUserAgent(_ ua: String) -> String {
        if ua.contains("presenter-receiver") { return "Presenter Receiver" }
        if ua.contains("Edg/") { return "Edge" }
        if ua.contains("Chrome/") && !ua.contains("Chromium") { return "Chrome" }
        if ua.contains("Firefox/") { return "Firefox" }
        if ua.contains("Safari/") && !ua.contains("Chrome/") { return "Safari" }
        return ua.count > 28 ? String(ua.prefix(28)) + "…" : ua
    }

    private func connectedDuration(_ iso: String) -> String? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: iso) ?? ISO8601DateFormatter().date(from: iso)
        guard let date else { return nil }
        let seconds = max(0, Int(Date.now.timeIntervalSince(date)))
        if seconds < 60 { return "\(seconds)s" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes)m" }
        let hours = minutes / 60
        return "\(hours)h \(minutes % 60)m"
    }

    private func latencyColor(_ ms: Int64) -> Color {
        if ms < 50 { return .green }
        if ms < 150 { return .orange }
        return .red
    }
}

#if os(macOS)
private typealias PlatformImage = NSImage
private extension Image {
    init(platformImage: NSImage) { self.init(nsImage: platformImage) }
}
#else
private typealias PlatformImage = UIImage
private extension Image {
    init(platformImage: UIImage) { self.init(uiImage: platformImage) }
}
#endif

/// The Raspberry Pi installer, with the same auto-start choice as the web UI's.
private struct PresenterInstallSection: View {
    let baseURL: String
    @State private var isAutoStarting = false
    @State private var didCopy = false

    private var command: String {
        let ws = baseURL
            .replacingOccurrences(of: "https://", with: "wss://")
            .replacingOccurrences(of: "http://", with: "ws://") + "/ws"
        let service = isAutoStarting ? " --service" : ""
        return "curl -fsSL https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh | bash -s -- \(ws)\(service)"
    }

    var body: some View {
        Section {
            Toggle("Auto-start on boot", isOn: $isAutoStarting)
            Text(command)
                .font(.caption.monospaced())
                .textSelection(.enabled)
            Button(didCopy ? "Copied" : "Copy Install Command", systemImage: "doc.on.doc") {
                #if os(macOS)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
                #else
                UIPasteboard.general.string = command
                #endif
                didCopy = true
            }
        } header: {
            Text("Presenter Receiver")
        } footer: {
            Text("Run this on the Raspberry Pi display. Auto-start sets up console auto-login on TTY1 (Linux / systemd only).")
        }
        .task(id: didCopy) {
            guard didCopy else { return }
            try? await Task.sleep(for: .seconds(2))
            didCopy = false
        }
    }
}

#if os(macOS)
/// The faces generated decks are written in. Keynote and PowerPoint show them only once they're
/// installed, which Font Book does after the person confirms.
private struct PresentationFontsSection: View {
    @State private var isInstalled = PresentationFonts.isInstalled
    @State private var failure: String?

    var body: some View {
        Section {
            LabeledContent("Cormorant Garamond, Geist Mono") {
                Text(isInstalled ? "Installed" : "Not installed")
                    .foregroundStyle(isInstalled ? .green : .orange)
            }
            Button(isInstalled ? "Install Again…" : "Install Fonts…", systemImage: "textformat") {
                do {
                    try PresentationFonts.openInFontBook()
                    failure = nil
                } catch {
                    failure = error.localizedDescription
                }
            }
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            }
        } header: {
            Text("Slide Fonts")
        } footer: {
            Text("Opens them in Font Book; choose Install there. Generated decks use these fonts.")
        }
        // Font Book installs them while this app is in the background.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            isInstalled = PresentationFonts.isInstalled
        }
    }
}

/// The same fonts and checks as the desktop app's `install_presentation_fonts`.
private enum PresentationFonts {
    static let files = ["CormorantGaramond-Regular.ttf", "GeistMono-Regular.ttf"]
    /// Earlier variable versions, which would shadow these.
    static let legacyFiles = ["Metocast-CormorantGaramond-Variable.ttf", "Metocast-GeistMono-Variable.ttf"]

    static var folder: URL { URL.libraryDirectory.appending(path: "Fonts") }

    static var isInstalled: Bool {
        files.allSatisfy { FileManager.default.fileExists(atPath: folder.appending(path: $0).path) }
    }

    static func openInFontBook() throws {
        let sources = try files.map { name in
            guard let url = Bundle.main.url(forResource: name, withExtension: nil) else {
                throw CocoaError(.fileNoSuchFile, userInfo: [NSLocalizedDescriptionKey: String(localized: "The app is missing its copy of \(name).")])
            }
            return url
        }
        for name in legacyFiles {
            let legacy = folder.appending(path: name)
            if FileManager.default.fileExists(atPath: legacy.path) {
                try FileManager.default.removeItem(at: legacy)
            }
        }
        guard let fontBook = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.FontBook") else {
            throw CocoaError(.fileNoSuchFile, userInfo: [NSLocalizedDescriptionKey: String(localized: "Font Book isn't on this Mac.")])
        }
        NSWorkspace.shared.open(sources, withApplicationAt: fontBook, configuration: NSWorkspace.OpenConfiguration())
    }
}
#endif
