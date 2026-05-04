import Foundation

struct Agent: Identifiable, Codable, Hashable {
    let id: String
    let name: String
    let agentType: AgentType
    let cwd: String?
    let activeCommand: String?

    enum AgentType: String, Codable, CaseIterable {
        case claude, codex, droid, other

        var iconName: String {
            switch self {
            case .claude: return "brain"
            case .codex: return "terminal"
            case .droid: return "gearshape.2"
            case .other: return "cpu"
            }
        }
    }
}
