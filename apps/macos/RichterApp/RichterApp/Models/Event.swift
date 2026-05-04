import Foundation

struct Event: Identifiable, Codable, Hashable {
    let id: String
    let kind: EventKind
    let title: String
    let summary: String
    let importance: Int
    let timestamp: Date
    let repoId: String?
    let agentId: String?

    var importanceTier: ImportanceTier {
        switch importance {
        case 0..<30: return .low
        case 30..<70: return .normal
        case 70..<90: return .high
        default: return .critical
        }
    }

    enum EventKind: String, Codable, CaseIterable {
        case runStarted = "run_started"
        case runCompleted = "run_completed"
        case runCached = "run_cached"
        case runQueued = "run_queued"
        case runDequeued = "run_dequeued"
        case testFailed = "test_failed"
        case buildError = "build_error"
        case resourcePressure = "resource_pressure"
        case agentConflict = "agent_conflict"
        case duplicateWorkSaved = "duplicate_work_saved"
        case info

        var iconName: String {
            switch self {
            case .runStarted: return "play.circle"
            case .runCompleted: return "checkmark.circle"
            case .runCached: return "clock.arrow.circlepath"
            case .runQueued: return "hourglass"
            case .runDequeued: return "arrow.up.circle"
            case .testFailed: return "xmark.octagon"
            case .buildError: return "exclamationmark.triangle"
            case .resourcePressure: return "gauge.medium"
            case .agentConflict: return "person.2.slash"
            case .duplicateWorkSaved: return "arrow.triangle.branch"
            case .info: return "info.circle"
            }
        }
    }

    enum ImportanceTier {
        case low, normal, high, critical
    }
}
