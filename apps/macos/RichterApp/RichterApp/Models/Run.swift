import Foundation

struct Run: Identifiable, Codable, Hashable {
    let id: String
    let command: String
    let repo: String
    let classification: String
    let status: RunStatus
    let exitCode: Int?
    let isCached: Bool
    let startTime: Date?
    let endTime: Date?

    var duration: TimeInterval? {
        guard let end = endTime, let start = startTime else { return nil }
        return end.timeIntervalSince(start)
    }

    enum RunStatus: String, Codable, CaseIterable {
        case queued
        case running
        case passed
        case failed
        case cancelled
        case timedOut = "timed_out"
        case cached
        case joined
        case unknown

        static func from(_ raw: String?) -> RunStatus {
            guard let raw = raw else { return .unknown }
            return RunStatus(rawValue: raw) ?? .unknown
        }

        var displayName: String {
            switch self {
            case .queued: return "Queued"
            case .running: return "Running"
            case .passed: return "Passed"
            case .failed: return "Failed"
            case .cancelled: return "Cancelled"
            case .timedOut: return "Timed Out"
            case .cached: return "Cached"
            case .joined: return "Joined"
            case .unknown: return "Unknown"
            }
        }

        var iconName: String {
            switch self {
            case .queued: return "clock"
            case .running: return "circle.dotted"
            case .passed: return "checkmark.circle.fill"
            case .failed: return "xmark.circle.fill"
            case .cancelled: return "slash.circle"
            case .timedOut: return "hourglass"
            case .cached: return "clock.arrow.circlepath"
            case .joined: return "arrow.triangle.merge"
            case .unknown: return "questionmark.circle"
            }
        }

        var color: String {
            switch self {
            case .queued: return "yellow"
            case .running: return "blue"
            case .passed, .cached, .joined: return "green"
            case .failed, .cancelled, .timedOut: return "red"
            case .unknown: return "gray"
            }
        }
    }
}
