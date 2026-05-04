import Foundation

struct Repo: Identifiable, Codable, Hashable {
    let id: String
    let name: String
    let root: String
    let branch: String?
    let isDirty: Bool
    let activeAgents: Int
    let activeRuns: Int
    let queuedRuns: Int
}
