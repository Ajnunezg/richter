import Foundation
import Network

// MARK: - Settings
struct RichterSettings: Codable {
    var watchedFolders: [String] = [NSHomeDirectory() + "/Projects"]
    var cacheTTLSeconds: Int = 300
    var retentionDays: Int = 30
    var shimEnabled: Bool = true
    var shellIntegrationEnabled: Bool = true
    var notificationsEnabled: Bool = true
    var importanceThreshold: Int = 50
    var coalesceWindowSeconds: Int = 5
    var modelProvider: String = ""
    var modelAPIKey: String = ""
    var cheapModel: String = ""
    var frontierModel: String = ""
    var payloadPreviewEnabled: Bool = false
    var redactAllSecrets: Bool = true
    var redactKeys: Bool = true
    var redactTokens: Bool = true
    var redactPasswords: Bool = true
    var redactURLs: Bool = true
    static let `default` = RichterSettings()
}

// MARK: - Response Types
struct HealthResponse: Codable {
    let status: String
    let timestamp: String
    let version: String
}

struct StatusResponse: Codable {
    let health: String
    let activeRuns: Int
    let queuedRuns: Int
    let cpuPercent: Double
    let memoryPercent: Double
    let subscriberCount: Int
    let cacheHitsToday: UInt64
    let duplicatesPrevented: UInt64
}

struct RunResponseItem: Codable {
    let runId: String
    let repo: String
    let command: String
    let classification: String
    let status: String?
    let exitCode: Int?
    let isActive: Bool
}

struct DaemonAgentInfo: Codable {
    let id: String
    let name: String
    let status: String
}

struct AuditEntryItem: Codable {
    let id: String
    let eventType: String
    let title: String
    let summary: String
    let severity: String
    let createdAt: String
}

struct AuditResponse: Codable {
    let entries: [AuditEntryItem]
    let total: Int
}

// MARK: - Error
enum DaemonClientError: LocalizedError {
    case connectionFailed(Error)
    case invalidResponse
    case decodingFailed(Error)
    case daemonNotRunning
    case unauthorized

    var errorDescription: String? {
        switch self {
        case .connectionFailed(let e): return "Daemon connection failed: \(e.localizedDescription)"
        case .invalidResponse: return "Invalid response from daemon"
        case .decodingFailed(let e): return "Failed to parse daemon response: \(e.localizedDescription)"
        case .daemonNotRunning: return "Richter daemon is not running"
        case .unauthorized: return "Authentication failed — check your Richter auth token"
        }
    }
}

// MARK: - Client
final class DaemonClient {
    static let shared = DaemonClient()

    private var socketPath: String {
        ProcessInfo.processInfo.environment["RICHTER_SOCKET"]
            ?? "/tmp/richter.sock"
    }

    private var authToken: String? {
        let tokenPath = "\(NSHomeDirectory())/.richter/auth_token"
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: tokenPath)),
              let token = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !token.isEmpty
        else { return nil }
        return token
    }

    func sendRequest<T: Decodable>(
        _ path: String,
        method: String = "GET",
        body: Data? = nil
    ) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            let connection = NWConnection(
                to: NWEndpoint.unix(path: socketPath),
                using: .tcp
            )
            var didResume = false
            connection.stateUpdateHandler = { [weak connection] state in
                guard !didResume else { return }
                switch state {
                case .ready:
                    didResume = true
                    var request = "\(method) \(path) HTTP/1.1\r\n"
                    request += "Host: localhost\r\n"
                    request += "Content-Type: application/json\r\n"
                    if let token = self.authToken {
                        request += "Authorization: Bearer \(token)\r\n"
                    }
                    if let body = body {
                        request += "Content-Length: \(body.count)\r\n"
                    }
                    request += "Connection: close\r\n\r\n"
                    var payload = request.data(using: .utf8)!
                    if let body = body { payload.append(body) }

                    connection?.send(content: payload, completion: .contentProcessed({ error in
                        if let error = error {
                            continuation.resume(throwing: DaemonClientError.connectionFailed(error))
                            connection?.cancel(); return
                        }
                        connection?.receive(minimumIncompleteLength: 1, maximumLength: 131072) { data, _, _, error in
                            if let error = error, (error as NSError).code != 57 && (error as NSError).code != 54 {
                                continuation.resume(throwing: DaemonClientError.connectionFailed(error))
                            } else if let data = data, let raw = String(data: data, encoding: .utf8) {
                                if let bs = raw.range(of: "\r\n\r\n") {
                                    let bodyStr = String(raw[bs.upperBound...])
                                    if let jd = bodyStr.data(using: .utf8) {
                                        do {
                                            let dec = try JSONDecoder().decode(T.self, from: jd)
                                            continuation.resume(returning: dec)
                                        } catch {
                                            continuation.resume(throwing: DaemonClientError.decodingFailed(error))
                                        }
                                    } else { continuation.resume(throwing: DaemonClientError.invalidResponse) }
                                } else { continuation.resume(throwing: DaemonClientError.invalidResponse) }
                            } else { continuation.resume(throwing: DaemonClientError.invalidResponse) }
                            connection?.cancel()
                        }
                    }))
                case .failed(let error):
                    didResume = true
                    continuation.resume(throwing: DaemonClientError.connectionFailed(error))
                    connection?.cancel()
                case .cancelled:
                    if !didResume { didResume = true; continuation.resume(throwing: DaemonClientError.daemonNotRunning) }
                default: break
                }
            }
            // Auto-cancel after 3 seconds if daemon unreachable
            let timeoutQueue = DispatchQueue.global()
            timeoutQueue.asyncAfter(deadline: .now() + 3.0) {
                if !didResume {
                    didResume = true
                    connection.cancel()
                    continuation.resume(throwing: DaemonClientError.daemonNotRunning)
                }
            }

            connection.start(queue: .global())
        }
    }

    func health() async throws -> HealthResponse { try await sendRequest("/health") }
    func getStatus() async throws -> StatusResponse { try await sendRequest("/status") }
    func getRuns() async throws -> [RunResponseItem] { try await sendRequest("/runs") }
    func getAgents() async throws -> [DaemonAgentInfo] { try await sendRequest("/agents") }
    func getAudit(last: Int = 50) async throws -> AuditResponse { try await sendRequest("/audit?last=\(last)") }
    func getSettings() async throws -> RichterSettings { try await sendRequest("/settings") }
    func isReachable() async -> Bool {
        do { let _: HealthResponse = try await sendRequest("/health"); return true }
        catch { return false }
    }
}
