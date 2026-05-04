import AppKit
import Combine

final class MenuBarController {
    private var statusItem: NSStatusItem!
    private weak var appState: AppState?
    private var cancellables = Set<AnyCancellable>()
    private var timer: Timer?

    init(state: AppState) {
        self.appState = state
        configureStatusItem()
        buildMenu()
        startObserving()
    }

    private func configureStatusItem() {
        print("[Richter] Creating status bar item…")
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        print("[Richter] Status item created: \(statusItem != nil ? "YES" : "NO")")
        if let button = statusItem.button {
            print("[Richter] Button frame: \(button.frame), visible: \(!button.isHidden)")
            print("[Richter] Setting icon…")
            button.image = NSImage(
                systemSymbolName: "circle.dotted",
                accessibilityDescription: "Richter"
            )
            button.toolTip = "Richter — agent control plane"
            print("[Richter] Icon set, tooltip configured")
        }
    }

    private func buildMenu() {
        let menu = NSMenu()

        // Header
        let header = NSMenuItem(title: "Richter", action: nil, keyEquivalent: "")
        header.isEnabled = false
        header.attributedTitle = NSAttributedString(
            string: "Richter",
            attributes: [.font: NSFont.boldSystemFont(ofSize: 13)]
        )
        menu.addItem(header)

        menu.addItem(.separator())

        // Status line
        let statusItem_menu = NSMenuItem(title: "Connecting…", action: nil, keyEquivalent: "")
        statusItem_menu.isEnabled = false
        statusItem_menu.identifier = NSUserInterfaceItemIdentifier(rawValue: "connectionStatus")
        menu.addItem(statusItem_menu)

        // Active/Queued
        let runsItem = NSMenuItem(title: "Active: —  Queued: —", action: nil, keyEquivalent: "")
        runsItem.isEnabled = false
        runsItem.identifier = NSUserInterfaceItemIdentifier(rawValue: "runCounts")
        menu.addItem(runsItem)

        menu.addItem(.separator())

        // Live events (last 3)
        let eventsLabel = NSMenuItem(title: "Recent:", action: nil, keyEquivalent: "")
        eventsLabel.isEnabled = false
        eventsLabel.attributedTitle = NSAttributedString(
            string: "Recent:",
            attributes: [.font: NSFont.boldSystemFont(ofSize: 11), .foregroundColor: NSColor.secondaryLabelColor]
        )
        menu.addItem(eventsLabel)

        for i in 0..<3 {
            let eventItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            eventItem.isEnabled = false
            eventItem.identifier = NSUserInterfaceItemIdentifier(rawValue: "event_\(i)")
            menu.addItem(eventItem)
        }

        menu.addItem(.separator())

        // Saved you
        let savedItem = NSMenuItem(title: "Saved: 0 runs today", action: nil, keyEquivalent: "")
        savedItem.isEnabled = false
        savedItem.identifier = NSUserInterfaceItemIdentifier(rawValue: "savedCounter")
        menu.addItem(savedItem)

        menu.addItem(.separator())

        // Actions
        let openItem = NSMenuItem(title: "Open Dashboard", action: #selector(openDashboard), keyEquivalent: "d")
        openItem.target = self
        menu.addItem(openItem)

        let runItem = NSMenuItem(title: "Run Command…", action: #selector(runCommand), keyEquivalent: "r")
        runItem.target = self
        menu.addItem(runItem)

        menu.addItem(.separator())

        let pauseItem = NSMenuItem(title: "Pause Coordination", action: #selector(togglePause), keyEquivalent: "p")
        pauseItem.target = self
        pauseItem.identifier = NSUserInterfaceItemIdentifier(rawValue: "pauseToggle")
        menu.addItem(pauseItem)

        menu.addItem(.separator())

        let startItem = NSMenuItem(title: "Start Daemon", action: #selector(startDaemon), keyEquivalent: "s")
        startItem.target = self
        startItem.identifier = NSUserInterfaceItemIdentifier(rawValue: "startDaemon")
        menu.addItem(startItem)

        let quitItem = NSMenuItem(title: "Quit Richter", action: #selector(quitApp), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)

        statusItem.menu = menu
    }

    private func startObserving() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
                self.refreshMenu()
            }
            if let t = self.timer {
                RunLoop.main.add(t, forMode: .common)
            }
        }
    }

    private func refreshMenu() {
        guard let state = appState, let menu = statusItem.menu else { return }

        // Connection status
        if let connItem = menu.items.first(where: { $0.identifier?.rawValue == "connectionStatus" }) {
            connItem.title = state.isDaemonConnected
                ? "Connected v\(state.daemonVersion)"
                : "Disconnected — start daemon"
        }

        // Show/hide start daemon item
        if let startItem = menu.items.first(where: { $0.identifier?.rawValue == "startDaemon" }) {
            startItem.isHidden = state.isDaemonConnected
        }

        // Run counts
        if let runsItem = menu.items.first(where: { $0.identifier?.rawValue == "runCounts" }) {
            runsItem.title = "Active: \(state.activeRuns)  Queued: \(state.queuedRuns)"
        }

        // Live events — show last 3
        let recent = state.decisions.prefix(3)
        for i in 0..<3 {
            if let item = menu.items.first(where: { $0.identifier?.rawValue == "event_\(i)" }) {
                if i < recent.count {
                    let d = recent[Array(recent.indices)[i]]
                    let prefix = severityIcon(d.severity)
                    item.title = "\(prefix) \(d.title)"
                    item.toolTip = d.summary
                } else {
                    item.title = "  —"
                    item.toolTip = ""
                }
            }
        }

        // Saved counter
        if let savedItem = menu.items.first(where: { $0.identifier?.rawValue == "savedCounter" }) {
            let total = state.cacheHitsToday + state.duplicatesPrevented
            savedItem.title = "Saved: \(total) runs today (\(state.cacheHitsToday) cache + \(state.duplicatesPrevented) dedup)"
        }

        // Pause toggle
        if let pauseItem = menu.items.first(where: { $0.identifier?.rawValue == "pauseToggle" }) {
            pauseItem.title = state.isPaused ? "Resume Coordination" : "Pause Coordination"
        }

        // Status icon
        updateStatusIcon()
    }

    private func updateStatusIcon() {
        guard let state = appState, let button = statusItem.button else { return }

        let symbolName: String
        if !state.isDaemonConnected {
            symbolName = "xmark.shield"
        } else if state.cpuPercent > 90 {
            symbolName = "exclamationmark.shield"
        } else if state.activeRuns > 0 {
            symbolName = "checkmark.shield"
        } else {
            symbolName = "circle.dotted"
        }

        button.image = NSImage(
            systemSymbolName: symbolName,
            accessibilityDescription: "Richter status"
        )
    }

    // MARK: - Actions

    @objc private func openDashboard() {
        NSApp.activate(ignoringOtherApps: true)
        for window in NSApp.windows {
            window.makeKeyAndOrderFront(nil)
            return
        }
    }

    @objc private func runCommand() {
        // Show a quick input dialog
        let alert = NSAlert()
        alert.messageText = "Run Command"
        alert.informativeText = "Enter a shell command to run through Richter:"
        alert.addButton(withTitle: "Run")
        alert.addButton(withTitle: "Cancel")

        let input = NSTextField(frame: NSRect(x: 0, y: 0, width: 300, height: 24))
        input.placeholderString = "e.g. cargo test"
        alert.accessoryView = input

        if alert.runModal() == .alertFirstButtonReturn {
            let cmd = input.stringValue
            if !cmd.isEmpty {
                // Launch CLI to run the command
                let task = Process()
                task.executableURL = URL(fileURLWithPath: "/usr/bin/env")
                task.arguments = ["richter", "run", "--", cmd]
                try? task.run()
            }
        }
    }

    @objc private func togglePause() {
        DispatchQueue.main.async { [weak self] in
            self?.appState?.isPaused.toggle()
        }
    }

    @objc private func startDaemon() {
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        task.arguments = ["richter-daemon"]
        try? task.run()
        // Update connection status on next poll
    }

    @objc private func quitApp() {
        NSApp.terminate(nil)
    }

    func cleanup() {
        timer?.invalidate()
        if let item = statusItem {
            NSStatusBar.system.removeStatusItem(item)
        }
    }
}

func severityIcon(_ severity: String) -> String {
    switch severity.lowercased() {
    case "critical": return "●"
    case "high": return "◉"
    case "warning": return "◑"
    case "info": return "○"
    default: return "·"
    }
}
