import SwiftUI

struct EventsView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                GlassCard {
                    HStack {
                        Image(systemName: "bell.badge.fill")
                            .font(.title2)
                            .foregroundColor(.orange)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Events")
                                .font(.headline)
                            Text("\(state.events.count) events tracked")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                        Spacer()
                    }
                }

                if state.events.isEmpty {
                    GlassCard {
                        VStack(spacing: 12) {
                            Image(systemName: "bell.slash")
                                .font(.largeTitle)
                                .foregroundColor(.secondary)
                            Text("No events yet")
                                .font(.headline)
                                .foregroundColor(.secondary)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                    }
                } else {
                    ForEach(state.events) { event in
                        EventRowView(event: event)
                    }
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}

struct EventRowView: View {
    let event: Event

    var body: some View {
        GlassCard {
            HStack(spacing: 8) {
                Text("\(event.importance)")
                    .font(.caption)
                    .fontWeight(.bold)
                    .foregroundColor(.white)
                    .frame(width: 28, height: 20)
                    .background(RoundedRectangle(cornerRadius: 4).fill(importanceColor))

                Image(systemName: event.kind.iconName)
                    .foregroundColor(importanceColor)

                VStack(alignment: .leading, spacing: 2) {
                    Text(event.title)
                        .font(.subheadline)
                        .lineLimit(1)
                    Text(event.timestamp, style: .relative)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .padding(.vertical, 2)
        }
    }

    private var importanceColor: Color {
        switch event.importanceTier {
        case .low: return .gray
        case .normal: return .blue
        case .high: return .orange
        case .critical: return .red
        }
    }
}
