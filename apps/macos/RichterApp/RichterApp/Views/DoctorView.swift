import SwiftUI
import AppKit

struct DoctorView: View {
    @EnvironmentObject var state: AppState
    @State private var isRunningDiagnostic = false
    @State private var results: DiagnosticResults?

    struct DiagnosticResults {
        var daemonOk: Bool = false
        var daemonPid: String = "unknown"
        var daemonUptime: String = "unknown"
        var shimStatus: [(String, Bool)] = []
        var hookStatus: [(String, Bool)] = []
        var mcpOk: Bool = false
        var mcpPort: String = "9777"
        var providerStatus: [(String, Bool)] = []
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Doctor")
                        .font(.title)
                        .fontWeight(.bold)
                    Spacer()
                    Button(action: runDiagnostic) {
                        if isRunningDiagnostic {
                            ProgressView()
                                .controlSize(.small)
                            Text("Running diagnostics…")
                        } else {
                            Image(systemName: "stethoscope")
                            Text("Run Diagnostic")
                        }
                    }
                    .disabled(isRunningDiagnostic)
                }

                if let r = results {
                    DiagnosticGroup(title: "Daemon", icon: "network") {
                        DiagnosticRow(label: "Status", value: r.daemonOk ? "Running" : "Not running", ok: r.daemonOk)
                        DiagnosticRow(label: "PID", value: r.daemonPid, ok: r.daemonOk)
                        DiagnosticRow(label: "Uptime", value: r.daemonUptime, ok: r.daemonOk)
                    }

                    DiagnosticGroup(title: "Shims", icon: "link") {
                        ForEach(r.shimStatus, id: \.0) { tool, ok in
                            DiagnosticRow(label: tool, value: ok ? "installed" : "missing", ok: ok)
                        }
                        if r.shimStatus.isEmpty {
                            Text("No shims installed. Run `richter install shims`.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                    }

                    DiagnosticGroup(title: "Hooks", icon: "gearshape.2") {
                        ForEach(r.hookStatus, id: \.0) { agent, ok in
                            DiagnosticRow(label: agent, value: ok ? "configured" : "not configured", ok: ok)
                        }
                    }

                    DiagnosticGroup(title: "MCP Server", icon: "antenna.radiowaves.left.and.right") {
                        DiagnosticRow(label: "Status", value: r.mcpOk ? "Listening" : "Not reachable", ok: r.mcpOk)
                        DiagnosticRow(label: "Port", value: r.mcpPort, ok: r.mcpOk)
                    }

                    DiagnosticGroup(title: "Permissions", icon: "lock.shield") {
                        PermissionRow(label: "Notifications", identifier: "x-apple.systempreferences:com.apple.preference.notifications")
                        PermissionRow(label: "Full Disk Access", identifier: "x-apple.systempreferences:com.apple.settings.Privacy")
                        PermissionRow(label: "Accessibility", identifier: "x-apple.systempreferences:com.apple.settings.Accessibility")
                    }

                    DiagnosticGroup(title: "Model Providers", icon: "brain") {
                        ForEach(r.providerStatus, id: \.0) { provider, ok in
                            DiagnosticRow(label: provider, value: ok ? "available" : "unavailable", ok: ok)
                        }
                    }
                } else if !isRunningDiagnostic {
                    Text("Press \"Run Diagnostic\" to check your Richter installation.")
                        .foregroundColor(.secondary)
                        .padding(.top, 40)
                }

                Spacer()
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }

    private func runDiagnostic() {
        isRunningDiagnostic = true
        results = nil

        Task {
            // Allow spinner to show briefly
            try? await Task.sleep(nanoseconds: 500_000_000)

            let daemonReachable = await DaemonClient.shared.isReachable()

            // Check shims by looking at ~/.richter/shims/
            let shimDir = "\(NSHomeDirectory())/.richter/shims"
            let fm = FileManager.default
            let shimTools = ["cargo", "npm", "pnpm", "yarn", "go", "python", "pytest", "make", "cmake", "bazel"]
            var shimStatus: [(String, Bool)] = []
            if fm.fileExists(atPath: shimDir) {
                for tool in shimTools {
                    shimStatus.append((tool, fm.fileExists(atPath: "\(shimDir)/\(tool)")))
                }
            }

            // Check hooks
            let hooksDir = "\(NSHomeDirectory())/.richter/hooks"
            var hookStatus: [(String, Bool)] = []
            for agent in ["claude", "codex", "droid"] {
                hookStatus.append((agent, fm.fileExists(atPath: "\(hooksDir)/\(agent).toml")))
            }

            // Check MCP
            let mcpOk = fm.fileExists(atPath: "\(NSHomeDirectory())/.richter/mcp.json")

            // Check for API keys (redacted check)
            var providerStatus: [(String, Bool)] = [
                ("OpenAI", ProcessInfo.processInfo.environment["OPENAI_API_KEY"] != nil),
                ("Anthropic", ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"] != nil),
                ("DeepSeek", ProcessInfo.processInfo.environment["DEEPSEEK_API_KEY"] != nil),
            ]

            let r = DiagnosticResults(
                daemonOk: daemonReachable,
                daemonPid: daemonReachable ? "running" : "N/A",
                daemonUptime: daemonReachable ? (try? await fetchDaemonUptime()) ?? "unknown" : "N/A",
                shimStatus: shimStatus,
                hookStatus: hookStatus,
                mcpOk: mcpOk,
                mcpPort: "9777",
                providerStatus: providerStatus
            )

            await MainActor.run {
                self.results = r
                self.isRunningDiagnostic = false
            }
        }
    }

    private func fetchDaemonUptime() async throws -> String {
        let resp: HealthResponse = try await DaemonClient.shared.sendRequest("/health")
        return resp.timestamp
    }
}

// MARK: - Subviews

struct DiagnosticGroup<Content: View>: View {
    let title: String
    let icon: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                Label(title, systemImage: icon)
                    .font(.headline)
                content()
            }
            .padding(4)
        }
    }
}

struct DiagnosticRow: View {
    let label: String
    let value: String
    let ok: Bool

    var body: some View {
        HStack {
            Image(systemName: ok ? "checkmark.circle.fill" : "xmark.circle.fill")
                .foregroundColor(ok ? .green : .red)
            Text(label)
                .foregroundColor(.secondary)
            Spacer()
            Text(value)
                .foregroundColor(ok ? .primary : .red)
        }
        .font(.callout)
    }
}

struct PermissionRow: View {
    let label: String
    let identifier: String

    var body: some View {
        HStack {
            Image(systemName: "circle")
                .foregroundColor(.orange)
            Text(label)
                .foregroundColor(.secondary)
            Spacer()
            Button("Open Settings") {
                if let url = URL(string: identifier) {
                    NSWorkspace.shared.open(url)
                }
            }
            .buttonStyle(.link)
        }
        .font(.callout)
    }
}
