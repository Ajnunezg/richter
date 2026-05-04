import SwiftUI

struct DecisionsView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                // Summary header
                GlassCard {
                    HStack {
                        Image(systemName: "brain.head.profile")
                            .font(.title2)
                            .foregroundColor(.purple)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Decision History")
                                .font(.headline)
                            Text("\(state.decisions.count) decisions tracked")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                        Spacer()
                        VStack(alignment: .trailing, spacing: 4) {
                            Label("\(state.cacheHitsToday) cache hits", systemImage: "bolt.fill")
                                .foregroundColor(.green)
                                .font(.caption)
                            Label("\(state.duplicatesPrevented) dupes saved", systemImage: "arrow.triangle.branch")
                                .foregroundColor(.blue)
                                .font(.caption)
                        }
                    }
                }

                // Decision list
                if state.decisions.isEmpty {
                    GlassCard {
                        VStack(spacing: 12) {
                            Image(systemName: "tray")
                                .font(.largeTitle)
                                .foregroundColor(.secondary)
                            Text("No decisions yet")
                                .font(.headline)
                                .foregroundColor(.secondary)
                            Text("Run commands through Richter and decisions will appear here.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                                .multilineTextAlignment(.center)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                    }
                } else {
                    ForEach(state.decisions) { decision in
                        DecisionRow(decision: decision)
                    }
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}

struct DecisionRow: View {
    let decision: DecisionRecord

    var body: some View {
        GlassCard {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: iconFor(decision.severity))
                    .foregroundColor(colorFor(decision.severity))
                    .font(.title3)

                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(decision.title)
                            .font(.subheadline)
                            .fontWeight(.medium)
                        Spacer()
                        Text(decision.timestamp)
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                    Text(decision.summary)
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .lineLimit(2)
                }
            }
            .padding(.vertical, 4)
        }
    }

    private func iconFor(_ severity: String) -> String {
        switch severity.lowercased() {
        case "critical", "high": return "exclamationmark.shield.fill"
        case "warning": return "exclamationmark.triangle.fill"
        case "info": return "info.circle.fill"
        default: return "circle.fill"
        }
    }

    private func colorFor(_ severity: String) -> Color {
        switch severity.lowercased() {
        case "critical": return .red
        case "high": return .orange
        case "warning": return .yellow
        case "info": return .blue
        default: return .gray
        }
    }
}
