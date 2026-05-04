import SwiftUI

struct SetupView: View {
    @EnvironmentObject var state: AppState

    @State private var steps: [OnboardingStep] = [
        OnboardingStep(title: "Daemon Running", done: false, action: "Start the Richter daemon"),
        OnboardingStep(title: "Database Created", done: false, action: "Your richter.db will be created automatically"),
        OnboardingStep(title: "Shims Configured", done: false, action: "Run: richter install shims"),
        OnboardingStep(title: "MCP Configured", done: false, action: "Run: richter install mcp"),
    ]

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                GlassCard {
                    VStack(spacing: 12) {
                        Image(systemName: "wand.and.stars")
                            .font(.largeTitle)
                            .foregroundColor(.accentColor)
                        Text("Welcome to Richter")
                            .font(.title2)
                            .fontWeight(.bold)
                        Text("Your agent coordination control plane")
                            .font(.subheadline)
                            .foregroundColor(.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 20)
                }

                GlassCard {
                    VStack(alignment: .leading, spacing: 16) {
                        Text("Setup Progress")
                            .font(.headline)

                        ForEach(steps.indices, id: \.self) { i in
                            HStack(spacing: 12) {
                                Image(systemName: steps[i].done ? "checkmark.circle.fill" : "circle")
                                    .foregroundColor(steps[i].done ? .green : .secondary)
                                    .font(.title3)

                                VStack(alignment: .leading, spacing: 2) {
                                    Text(steps[i].title)
                                        .font(.subheadline)
                                    if !steps[i].done {
                                        Text(steps[i].action)
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                }
                                Spacer()
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }

                if state.isDaemonConnected {
                    GlassCard {
                        HStack {
                            Image(systemName: "checkmark.seal.fill")
                                .foregroundColor(.green)
                                .font(.title2)
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Connected to Daemon")
                                    .font(.headline)
                                Text("Version \(state.daemonVersion)")
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                            Spacer()
                        }
                    }

                    Button("Copy Setup Commands") {
                        let cmds = """
                        richter install shims
                        richter install mcp
                        richter setup --all
                        """
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(cmds, forType: .string)
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}

struct OnboardingStep {
    let title: String
    var done: Bool
    let action: String
}
