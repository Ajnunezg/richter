import SwiftUI

struct ContentView: View {
    @EnvironmentObject var state: AppState
    @State private var selectedTab: DashboardTab = .now
    @State private var showRunCommand: Bool = false
    @State private var runCommandText: String = ""

    enum DashboardTab: String, CaseIterable {
        case now = "Now"
        case runs = "Runs"
        case decisions = "Decisions"
        case events = "Events"
        case repos = "Repos"
        case agents = "Agents"
        case setup = "Setup"
        case doctor = "Doctor"
        case settings = "Settings"

        var icon: String {
            switch self {
            case .now: return "clock"
            case .runs: return "list.bullet"
            case .decisions: return "brain.head.profile"
            case .events: return "bell.badge"
            case .repos: return "folder"
            case .agents: return "person.2"
            case .setup: return "wand.and.stars"
            case .doctor: return "stethoscope"
            case .settings: return "gearshape"
            }
        }
    }

    var body: some View {
        NavigationSplitView {
            // Sidebar — tab selection
            List(DashboardTab.allCases, id: \.self, selection: $selectedTab) { tab in
                Label(tab.rawValue, systemImage: tab.icon)
                    .padding(.vertical, 2)
            }
            .listStyle(.sidebar)
            .frame(minWidth: 180)
        } detail: {
            // Main content area
            VStack(spacing: 0) {
                // Status bar
                HStack(spacing: 16) {
                    HStack(spacing: 4) {
                        Circle()
                            .fill(state.isDaemonConnected ? .green : .red)
                            .frame(width: 8, height: 8)
                        Text(state.isDaemonConnected ? "Connected" : "Disconnected")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Divider().frame(height: 16)

                    Label("\(state.activeRuns) active", systemImage: "circle.dotted")
                        .font(.caption).foregroundColor(.secondary)
                    Label("\(state.queuedRuns) queued", systemImage: "hourglass")
                        .font(.caption).foregroundColor(.secondary)

                    Spacer()

                    Label("\(state.cacheHitsToday + state.duplicatesPrevented) saved today", systemImage: "heart.fill")
                        .font(.caption)
                        .foregroundColor(.pink)

                    Button(action: { showRunCommand = true }) {
                        Image(systemName: "terminal")
                            .font(.caption)
                    }
                    .buttonStyle(.borderless)
                    .help("Run Command")
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(.bar)

                Divider()

                // Tab content
                Group {
                    switch selectedTab {
                    case .now: NowDashboard()
                    case .runs: RunsDashboard()
                    case .decisions: DecisionsDashboard()
                    case .events: EventsDashboard()
                    case .repos: ReposView()
                    case .agents: AgentsView()
                    case .setup: SetupView()
                    case .doctor: DoctorView()
                    case .settings: SettingsView()
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 900, minHeight: 600)
        .sheet(isPresented: $showRunCommand) {
            RunCommandSheet(isPresented: $showRunCommand)
        }
    }
}

// MARK: - Run Command Sheet

struct RunCommandSheet: View {
    @Binding var isPresented: Bool
    @State private var command: String = ""
    @State private var repo: String = "."

    var body: some View {
        VStack(spacing: 20) {
            Text("Run Command")
                .font(.title2)
                .fontWeight(.semibold)

            TextField("Command", text: $command)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))

            HStack {
                Text("Repo:")
                    .foregroundColor(.secondary)
                TextField(".", text: $repo)
                    .textFieldStyle(.roundedBorder)
            }

            HStack(spacing: 12) {
                Button("Cancel") { isPresented = false }
                    .keyboardShortcut(.escape)

                Button("Run") {
                    let task = Process()
                    task.executableURL = URL(fileURLWithPath: "/usr/bin/env")
                    task.arguments = ["richter", "run", "--", command]
                    try? task.run()
                    isPresented = false
                }
                .keyboardShortcut(.return)
                .disabled(command.isEmpty)
            }
        }
        .padding(30)
        .frame(width: 420)
    }
}

// MARK: - Dashboard Views (compact, live-updating)

struct NowDashboard: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                // Pressure gauges
                HStack(spacing: 16) {
                    MetricCard(
                        title: "CPU", value: "\(Int(state.cpuPercent))%",
                        icon: "cpu", color: state.cpuPercent > 75 ? .red : .green
                    )
                    MetricCard(
                        title: "Memory", value: "\(Int(state.memoryPercent))%",
                        icon: "memorychip", color: state.memoryPercent > 85 ? .red : .green
                    )
                    MetricCard(
                        title: "Active", value: "\(state.activeRuns)",
                        icon: "circle.dotted", color: .blue
                    )
                    MetricCard(
                        title: "Queued", value: "\(state.queuedRuns)",
                        icon: "hourglass", color: .orange
                    )
                }

                // Live runs
                VStack(alignment: .leading, spacing: 8) {
                    Label("Active Runs", systemImage: "terminal")
                        .font(.headline)

                    if state.runs.filter({ $0.status == .running }).isEmpty {
                        Text("No active runs")
                            .foregroundColor(.secondary)
                            .padding(.vertical, 20)
                            .frame(maxWidth: .infinity)
                    } else {
                        ForEach(state.runs.filter { $0.status == .running }.prefix(10)) { run in
                            HStack(spacing: 8) {
                                Image(systemName: "circle.dotted")
                                    .foregroundColor(.blue)
                                Text(run.command)
                                    .font(.subheadline)
                                    .lineLimit(1)
                                Spacer()
                                Text(run.classification)
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 2)
                                    .background(Capsule().fill(.secondary.opacity(0.15)))
                            }
                            .padding(.vertical, 3)
                        }
                    }
                }
            }
            .padding()
        }
    }
}

struct RunsDashboard: View {
    @EnvironmentObject var state: AppState
    @State private var filter: String = ""

    var body: some View {
        VStack(spacing: 0) {
            TextField("Filter runs…", text: $filter)
                .textFieldStyle(.roundedBorder)
                .padding()

            List(state.runs.filter { filter.isEmpty || $0.command.contains(filter) }) { run in
                HStack(spacing: 8) {
                    Image(systemName: run.status.iconName)
                        .foregroundColor(run.status.color == "red" ? .red : run.status.color == "green" ? .green : .blue)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(run.command).font(.subheadline)
                        Text(run.classification).font(.caption).foregroundColor(.secondary)
                    }
                    Spacer()
                    Text(run.status.displayName)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                .padding(.vertical, 3)
            }
        }
    }
}

struct DecisionsDashboard: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        List(state.decisions) { d in
            HStack(spacing: 8) {
                Image(systemName: severityIcon(d.severity))
                    .foregroundColor(severityColor(d.severity))
                VStack(alignment: .leading, spacing: 2) {
                    Text(d.title).font(.subheadline).fontWeight(.medium)
                    Text(d.summary).font(.caption).foregroundColor(.secondary)
                }
                Spacer()
                Text(d.timestamp).font(.caption).foregroundColor(.secondary)
            }
            .padding(.vertical, 2)
        }
    }

    func severityIcon(_ s: String) -> String {
        switch s.lowercased() {
        case "critical": return "exclamationmark.shield.fill"
        case "high": return "exclamationmark.triangle.fill"
        case "warning": return "exclamationmark.triangle"
        default: return "info.circle"
        }
    }

    func severityColor(_ s: String) -> Color {
        switch s.lowercased() {
        case "critical": return .red
        case "high": return .orange
        case "warning": return .yellow
        default: return .blue
        }
    }
}

struct EventsDashboard: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        List(state.events) { event in
            HStack(spacing: 8) {
                Text("\(event.importance)")
                    .font(.caption).fontWeight(.bold)
                    .foregroundColor(.white)
                    .frame(width: 24, height: 18)
                    .background(RoundedRectangle(cornerRadius: 4).fill(
                        event.importance >= 70 ? .red : event.importance >= 50 ? .orange : .blue
                    ))
                Image(systemName: event.kind.iconName)
                VStack(alignment: .leading, spacing: 2) {
                    Text(event.title).font(.subheadline)
                    Text(event.summary).font(.caption).foregroundColor(.secondary)
                }
            }
            .padding(.vertical, 2)
        }
    }
}

// MARK: - Shared Components

struct MetricCard: View {
    let title: String
    let value: String
    let icon: String
    let color: Color

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: icon)
                .font(.title2)
                .foregroundColor(color)
            Text(value)
                .font(.title2)
                .fontWeight(.bold)
                .monospacedDigit()
            Text(title)
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
        .background(RoundedRectangle(cornerRadius: 10).fill(.regularMaterial))
    }
}
