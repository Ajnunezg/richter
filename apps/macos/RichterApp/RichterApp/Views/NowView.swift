import SwiftUI

struct NowView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                systemPressureSection
                activeRunsSection
                duplicateWorkSection
                importantEventsSection
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }

    // MARK: - System Pressure

    private var systemPressureSection: some View {
        HStack(spacing: 16) {
            PressureGaugeCard(
                icon: "cpu",
                title: "CPU",
                value: Double(state.cpuPercent),
                suffix: "%",
                color: Double(state.cpuPercent) > 75 ? .red : Double(state.cpuPercent) > 50 ? .orange : .green,
                maxValue: 100
            )
            PressureGaugeCard(
                icon: "memorychip",
                title: "Memory",
                value: Double(state.memoryPercent),
                suffix: " GB",
                color: memoryColor,
                maxValue: 100,
                formatter: { String(format: "%.0f%%", $0) }
            )
        
        }
    }

    private var memoryColor: Color {
        let ratio = Double(state.memoryPercent) / 100.0
        return ratio > 0.85 ? .red : ratio > 0.65 ? .orange : .green
    }

    private var diskColor: Color { .green }

    // MARK: - Active Runs

    private var activeRunsSection: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 12) {
                Label("Active Heavy Runs", systemImage: "hammer")
                    .font(.headline)

                if state.activeRunsList.isEmpty {
                    Text("No active runs")
                        .foregroundColor(.secondary)
                        .padding(.vertical, 8)
                } else {
                    ForEach(state.runs.filter { $0.status == .running }.prefix(8)) { run in
                        RunRowView(run: run)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    // MARK: - Duplicate Work Saved

    private var duplicateWorkSection: some View {
        GlassCard {
            HStack {
                Image(systemName: "arrow.triangle.branch")
                    .font(.title)
                    .foregroundColor(.blue)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Duplicate Work Saved")
                        .font(.headline)
                    Text("\(Int(state.duplicatesPrevented)) redundant operations cached")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                }
                Spacer()
                Text("\(Int(state.duplicatesPrevented))")
                    .font(.largeTitle)
                    .fontWeight(.bold)
                    .foregroundColor(.blue)
            }
        }
    }

    // MARK: - Important Events

    private var importantEventsSection: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 10) {
                Label("Important Events", systemImage: "bell.badge")
                    .font(.headline)

                if state.importantEvents.isEmpty {
                    Text("No important events")
                        .foregroundColor(.secondary)
                        .padding(.vertical, 8)
                } else {
                    ForEach(state.importantEvents.prefix(5)) { event in
                        EventRowView(event: event)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: - Subviews

struct PressureGaugeCard: View {
    let icon: String
    let title: String
    let value: Double
    let suffix: String
    let color: Color
    let maxValue: Double
    var formatter: ((Double) -> String)?

    var body: some View {
        GlassCard {
            VStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.title2)
                    .foregroundColor(color)

                Text(title)
                    .font(.caption)
                    .foregroundColor(.secondary)

                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 6)
                        .fill(color.opacity(0.15))
                        .frame(height: 8)

                    RoundedRectangle(cornerRadius: 6)
                        .fill(color)
                        .frame(width: gaugeWidth, height: 8)
                        .animation(.easeInOut(duration: 0.5), value: value)
                }

                Text(formattedValue)
                    .font(.title3)
                    .fontWeight(.semibold)
                    .monospacedDigit()
            }
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity)
        }
    }

    private var ratio: Double {
        min(value / maxValue, 1.0)
    }

    private var gaugeWidth: CGFloat {
        let base: CGFloat = 120
        return base * ratio
    }

    private var formattedValue: String {
        if let f = formatter {
            return f(value)
        }
        return String(format: "%.0f%@", value, suffix)
    }
}

struct RunRowView: View {
    let run: Run

    var body: some View {
        HStack(spacing: 8) {
            statusIcon
                .foregroundColor(statusColor)

            VStack(alignment: .leading, spacing: 2) {
                Text(run.command)
                    .font(.subheadline)
                    .lineLimit(1)

                if let duration = run.duration {
                    Text(durationFormatted(duration))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            Spacer()

            if run.isCached {
                Label("Cached", systemImage: "clock.arrow.circlepath")
                    .font(.caption)
                    .foregroundColor(.green)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(.green.opacity(0.15)))
            }

            Text(run.classification)
                .font(.caption)
                .foregroundColor(.blue)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Capsule().fill(.blue.opacity(0.15)))
        }
        .padding(.vertical, 2)
    }

    private var statusIcon: Image {
        Image(systemName: run.status.iconName)
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


    private func durationFormatted(_ d: TimeInterval) -> String {
        if d < 60 { return String(format: "%.0fs", d) }
        if d < 3600 { return String(format: "%.1fm", d / 60) }
        return String(format: "%.1fh", d / 3600)
    }
}



// MARK: - Glass Card

struct GlassCard<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .padding()
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .fill(.regularMaterial)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(.white.opacity(0.15), lineWidth: 0.5)
            )
    }
}
