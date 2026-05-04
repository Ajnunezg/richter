import SwiftUI

struct ReposView: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                GlassCard {
                    HStack {
                        Image(systemName: "folder.fill")
                            .font(.title2)
                            .foregroundColor(.blue)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Repositories")
                                .font(.headline)
                            Text("\(state.repos.count) tracked")
                                .font(.subheadline)
                                .foregroundColor(.secondary)
                        }
                        Spacer()
                    }
                }

                ForEach(state.repos) { repo in
                    GlassCard {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(repo.name)
                                .font(.subheadline)
                                .fontWeight(.medium)
                            Text(repo.root)
                                .font(.caption)
                                .foregroundColor(.secondary)
                            HStack(spacing: 12) {
                                if let branch = repo.branch {
                                    Label(branch, systemImage: "arrow.triangle.branch")
                                        .font(.caption)
                                        .foregroundColor(.secondary)
                                }
                                Label("\(repo.activeAgents) agents", systemImage: "person")
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                                Label("\(repo.activeRuns) runs", systemImage: "circle.dotted")
                                    .font(.caption)
                                    .foregroundColor(.secondary)
                            }
                        }
                    }
                }
            }
            .padding()
        }
        .background(.ultraThinMaterial)
    }
}
