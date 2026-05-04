import SwiftUI

struct RunsView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                GlassCard {
                    HStack {
                        Image(systemName: "list.bullet.clipboard")
                            .font(.title2)
                            .foregroundColor(.blue)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Active Runs")
                                .font(.headline)
                            Text("\(state.runs.count) total, \(state.activeRuns) running")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                        Spacer()
                    }
                }

                if state.runs.isEmpty {
                    GlassCard {
                        VStack(spacing: 12) {
                            Image(systemName: "terminal")
                                .font(.largeTitle)
                                .foregroundColor(.secondary)
                            Text("No runs yet")
                                .font(.headline)
                                .foregroundColor(.secondary)
                            Text("Submit commands via the CLI or MCP server and they will appear here.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                                .multilineTextAlignment(.center)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                    }
                } else {
                    ForEach(state.runs) { run in
                        RunDetailCard(run: run)
                    }
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}

struct RunDetailCard: View {
    let run: Run

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Image(systemName: run.status.iconName)
                        .foregroundColor(statusColor)
                    Text(run.command)
                        .font(.subheadline)
                        .fontWeight(.medium)
                        .lineLimit(1)
                    Spacer()
                    Text(run.status.displayName)
                        .font(.caption)
                        .foregroundColor(statusColor)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(statusColor.opacity(0.15)))
                }

                HStack(spacing: 16) {
                    Label(run.repo, systemImage: "folder")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Label(run.classification, systemImage: "tag")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    if let code = run.exitCode {
                        Label("exit \(code)", systemImage: "arrow.turn.up.left")
                            .font(.caption)
                            .foregroundColor(code == 0 ? .green : .red)
                    }
                }
            }
        }
    }

    private var statusColor: Color {
        switch run.status.color {
        case "blue": return .blue
        case "green": return .green
        case "red": return .red
        case "yellow": return .yellow
        case "orange": return .orange
        default: return .gray
        }
    }
}
