import SwiftUI
import UserNotifications
import ServiceManagement

@main
struct RichterApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate
    @StateObject private var appState = AppState()

    var body: some Scene {
        WindowGroup("Richter Dashboard", id: "dashboard") {
            ContentView()
                .environmentObject(appState)
                .frame(minWidth: 900, minHeight: 600)
                .onAppear {
                    appDelegate.appState = appState
                }
        }
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unified)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Dashboard Window") {
                    if let firstWindow = NSApp.windows.first {
                        firstWindow.makeKeyAndOrderFront(nil)
                    }
                }
                .keyboardShortcut("n", modifiers: .command)
            }
            CommandGroup(replacing: .help) {
                Button("Richter Help") {
                    if let url = URL(string: "https://github.com/ajnunezg/richter") {
                        NSWorkspace.shared.open(url)
                    }
                }
            }
        }

        Settings {
            SettingsView()
                .environmentObject(appState)
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    weak var appState: AppState?
    private var menuBarController: MenuBarController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }

        NSApp.setActivationPolicy(.regular)

        if #available(macOS 14.0, *) {
            do {
                try SMAppService.mainApp.register()
            } catch {
                print("SMAppService registration failed: \(error)")
            }
        }

        DispatchQueue.main.async { [weak self] in
            guard let self, let state = self.appState else { return }
            let controller = MenuBarController(state: state)
            self.menuBarController = controller
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        menuBarController?.cleanup()
    }
}
