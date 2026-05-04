import SwiftUI

struct AgentsView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                GlassCard {
                    HStack {
                        Image(systemName: "person.2.fill")
                            .font(.title2)
                            .foregroundColor(.purple)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Agents")
                                .font(.headline)
                            Text("\(state.agents.count) detected")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                        Spacer()
                    }
                }

                ForEach(state.agents) { agent in
                    GlassCard {
                        HStack(spacing: 12) {
                            Image(systemName: agent.agentType.iconName)
                                .font(.title2)
                                .foregroundColor(.purple)

                            VStack(alignment: .leading, spacing: 4) {
                                Text(agent.name)
                                    .font(.subheadline)
                                    .fontWeight(.medium)
                                if let cmd = agent.activeCommand {
                                    Text(cmd)
                                        .font(.caption)
                                        .foregroundColor(.secondary)
                                        .lineLimit(1)
                                }
                            }
                            Spacer()
                        }
                    }
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}
