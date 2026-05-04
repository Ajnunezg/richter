import Foundation
import Combine

final class AppState: ObservableObject {
    @Published var isDaemonConnected: Bool = false
    @Published var daemonVersion: String = ""

    @Published var cpuPercent: Float = 0
    @Published var memoryPercent: Float = 0
    @Published var activeRuns: Int = 0
    @Published var queuedRuns: Int = 0
    @Published var cacheHitsToday: UInt64 = 0
    @Published var duplicatesPrevented: UInt64 = 0
    @Published var subscriberCount: Int = 0

    @Published var repos: [Repo] = []
    @Published var agents: [Agent] = []
    @Published var runs: [Run] = []
    @Published var events: [Event] = []
    @Published var decisions: [DecisionRecord] = []
    @Published var settings: RichterSettings = .default

    @Published var isPaused: Bool = false
    @Published var notificationsEnabled: Bool = true
    @Published var shimsEnabled: Bool = true

    private let client = DaemonClient.shared
    private var pollTask: Task<Void, Never>?

    init() {
        startRealPolling()
    }

    private func startRealPolling() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                do {
                    let health = try await client.health()
                    let status = try await client.getStatus()
                    let runs = try await client.getRuns()
                    let audit = try await client.getAudit(last: 50)
                    let agentList: [DaemonAgentInfo] = try await client.getAgents()

                    let mappedRuns = runs.map { r in
                        Run(id: r.runId, command: r.command, repo: r.repo,
                            classification: r.classification,
                            status: Run.RunStatus.from(r.status),
                            exitCode: r.exitCode, isCached: false,
                            startTime: nil, endTime: nil)
                    }
                    let mappedDecisions = audit.entries.map { e in
                        DecisionRecord(id: e.id, title: e.title, summary: e.summary,
                                       severity: e.severity, timestamp: e.createdAt)
                    }
                    let mappedAgents = agentList.map { a in
                        Agent(id: a.id, name: a.name, agentType: .other, cwd: nil, activeCommand: nil)
                    }

                    DispatchQueue.main.async {
                        self.isDaemonConnected = true
                        self.daemonVersion = health.version
                        self.activeRuns = status.activeRuns
                        self.queuedRuns = status.queuedRuns
                        self.cpuPercent = Float(status.cpuPercent)
                        self.memoryPercent = Float(status.memoryPercent)
                        self.subscriberCount = status.subscriberCount
                        self.cacheHitsToday = status.cacheHitsToday
                        self.duplicatesPrevented = status.duplicatesPrevented
                        self.runs = mappedRuns
                        self.decisions = mappedDecisions
                        self.agents = mappedAgents
                    }
                } catch {
                    DispatchQueue.main.async {
                        self.isDaemonConnected = false
                        self.daemonVersion = ""
                    }
                    // Brief pause before retry to avoid tight crash loops
                    try? await Task.sleep(nanoseconds: 2_000_000_000)
                }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }

    var activeRunsList: [Run] { runs.filter { $0.status == .running } }
    var importantEvents: [Event] { events.filter { $0.importance >= 50 }.sorted { $0.importance > $1.importance } }
}

struct DecisionRecord: Identifiable {
    let id: String
    let title: String
    let summary: String
    let severity: String
    let timestamp: String
}
