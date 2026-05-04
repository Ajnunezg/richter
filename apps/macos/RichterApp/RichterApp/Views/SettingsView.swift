import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var state: AppState
    @State private var selectedTab = "general"
    @State private var editedSettings = RichterSettings.default

    private let tabs = ["general", "integrations", "models", "privacy", "notifications"]

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $selectedTab) {
                Text("General").tag("general")
                Text("Integrations").tag("integrations")
                Text("Models").tag("models")
                Text("Privacy").tag("privacy")
                Text("Notifications").tag("notifications")
            }
            .pickerStyle(.segmented)
            .padding()

            ScrollView {
                switch selectedTab {
                case "general": generalTab
                case "integrations": integrationsTab
                case "models": modelsTab
                case "privacy": privacyTab
                case "notifications": notificationsTab
                default: EmptyView()
                }
            }
        }
        .frame(width: 520, height: 440)
        .onAppear {
            editedSettings = state.settings
        }
    }

    // MARK: - General

    private var generalTab: some View {
        Form {
            Section("Watched Folders") {
                ForEach(Array(editedSettings.watchedFolders.enumerated()), id: \.offset) { _, folder in
                    HStack {
                        Text(folder)
                            .font(.caption)
                        Spacer()
                        Button("Remove") {
                            editedSettings.watchedFolders.removeAll { $0 == folder }
                        }
                        .buttonStyle(.link)
                    }
                }
                Button("Add Folder…") {
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.canChooseFiles = false
                    if panel.runModal() == .OK, let url = panel.url {
                        editedSettings.watchedFolders.append(url.path)
                    }
                }
            }
            Section("Caching") {
                Slider(value: Binding(
                    get: { Double(editedSettings.cacheTTLSeconds) },
                    set: { editedSettings.cacheTTLSeconds = Int($0) }
                ), in: 0...3600) {
                    Text("Cache TTL: \(editedSettings.cacheTTLSeconds)s")
                }
            }
            Section("Retention") {
                Slider(value: Binding(
                    get: { Double(editedSettings.retentionDays) },
                    set: { editedSettings.retentionDays = Int($0) }
                ), in: 1...365) {
                    Text("Retain logs: \(editedSettings.retentionDays)d")
                }
            }
        }
        .padding()
    }

    // MARK: - Integrations

    private var integrationsTab: some View {
        Form {
            Section("Shims & Shell") {
                Toggle("Shims enabled (~/.richter/shims)", isOn: $editedSettings.shimEnabled)
                Toggle("Shell integration active", isOn: $editedSettings.shellIntegrationEnabled)
            }
            Section("Agent Hooks") {
                HStack {
                    Text("Claude Code")
                    Spacer()
                    Image(systemName: "checkmark.circle.fill").foregroundColor(.green)
                    Button("Reinstall") {}.buttonStyle(.link)
                }
                HStack {
                    Text("OpenAI Codex")
                    Spacer()
                    Image(systemName: "xmark.circle.fill").foregroundColor(.red)
                    Button("Install") {}.buttonStyle(.link)
                }
                HStack {
                    Text("Droid")
                    Spacer()
                    Image(systemName: "checkmark.circle.fill").foregroundColor(.green)
                    Button("Reinstall") {}.buttonStyle(.link)
                }
            }
            Section("MCP") {
                Toggle("MCP server enabled", isOn: .constant(true))
                Text("Agents connect via MCP at localhost:9777")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
        .padding()
    }

    // MARK: - Models

    private var modelsTab: some View {
        Form {
            Section("Provider") {
                Picker("LLM Provider", selection: $editedSettings.modelProvider) {
                    Text("None").tag("")
                    Text("OpenAI").tag("openai")
                    Text("Anthropic").tag("anthropic")
                    Text("DeepSeek").tag("deepseek")
                    Text("Ollama (local)").tag("ollama")
                }
                if !editedSettings.modelProvider.isEmpty {
                    SecureField("API Key", text: $editedSettings.modelAPIKey)
                }
            }
            if !editedSettings.modelProvider.isEmpty {
                Section("Cheap Classifier") {
                    TextField("Model ID", text: $editedSettings.cheapModel)
                        .textContentType(.none)
                }
                Section("Frontier Adjudicator") {
                    TextField("Model ID", text: $editedSettings.frontierModel)
                        .textContentType(.none)
                }
                Section("Debug") {
                    Toggle("LLM payload preview", isOn: $editedSettings.payloadPreviewEnabled)
                    Text("Preview shows what data would be sent to the model provider (with secrets redacted).")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
        }
        .padding()
    }

    // MARK: - Privacy

    private var privacyTab: some View {
        Form {
            Section("Secrets Redaction") {
                Toggle("Redact all secrets", isOn: $editedSettings.redactAllSecrets)
                Toggle("Redact API keys", isOn: $editedSettings.redactKeys)
                    .disabled(!editedSettings.redactAllSecrets)
                Toggle("Redact bearer tokens", isOn: $editedSettings.redactTokens)
                    .disabled(!editedSettings.redactAllSecrets)
                Toggle("Redact passwords", isOn: $editedSettings.redactPasswords)
                    .disabled(!editedSettings.redactAllSecrets)
                Toggle("Redact database URLs", isOn: $editedSettings.redactURLs)
                    .disabled(!editedSettings.redactAllSecrets)
            }
            Section("Data") {
                Text("All data stays on your machine. No telemetry is sent unless you explicitly configure a model provider.")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
        .padding()
    }

    // MARK: - Notifications

    private var notificationsTab: some View {
        Form {
            Section("Alerts") {
                Toggle("Enable notifications", isOn: $editedSettings.notificationsEnabled)
                Slider(value: Binding(
                    get: { Double(editedSettings.importanceThreshold) },
                    set: { editedSettings.importanceThreshold = Int($0) }
                ), in: 0...100) {
                    Text("Minimum importance: \(editedSettings.importanceThreshold)%")
                }
                Slider(value: Binding(
                    get: { Double(editedSettings.coalesceWindowSeconds) },
                    set: { editedSettings.coalesceWindowSeconds = Int($0) }
                ), in: 1...60) {
                    Text("Coalesce window: \(editedSettings.coalesceWindowSeconds)s")
                }
            }
        }
        .padding()
    }
}
