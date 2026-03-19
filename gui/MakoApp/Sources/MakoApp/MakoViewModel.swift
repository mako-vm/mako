import Foundation
import Combine

struct ContainerInfo: Identifiable, Codable {
    var id: String { Id }
    let Id: String
    let Names: [String]
    let Image: String
    let State: String
    let Status: String

    var displayName: String {
        Names.first?.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? Id.prefix(12).description
    }
    var isRunning: Bool { State == "running" }
}

class MakoViewModel: ObservableObject {
    @Published var isRunning = false
    @Published var containers: [ContainerInfo] = []
    @Published var cpuUsage: String = "—"
    @Published var memoryUsage: String = "—"
    @Published var dockerVersion: String = "—"
    @Published var uptime: String = "—"
    @Published var statusMessage: String?

    private var timer: Timer?
    private let socketPath: String

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        socketPath = "\(home)/.mako/docker.sock"
    }

    func startPolling() {
        poll()
        timer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    func poll() {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            let running = self.isDaemonAlive()
            DispatchQueue.main.async {
                self.isRunning = running
            }
            if running {
                self.fetchContainers()
                self.fetchVersion()
            }
        }
    }

    func startMako() {
        guard let path = findMakoBinary() else {
            DispatchQueue.main.async {
                self.statusMessage = "Cannot find mako binary"
            }
            return
        }

        DispatchQueue.main.async {
            self.statusMessage = "Starting Mako..."
        }

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: path)
            process.arguments = ["start"]
            process.currentDirectoryURL = URL(fileURLWithPath: (path as NSString).deletingLastPathComponent)

            let errPipe = Pipe()
            process.standardOutput = Pipe()
            process.standardError = errPipe

            do {
                try process.run()
                process.waitUntilExit()

                if process.terminationStatus != 0 {
                    let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
                    let errStr = String(data: errData, encoding: .utf8) ?? "unknown error"
                    DispatchQueue.main.async {
                        self.statusMessage = "Start failed: \(errStr.prefix(200))"
                    }
                    return
                }
            } catch {
                DispatchQueue.main.async {
                    self.statusMessage = "Start error: \(error.localizedDescription)"
                }
                return
            }

            // VM takes ~15s to boot; poll every 3s
            DispatchQueue.main.async {
                self.statusMessage = "VM booting..."
            }
            for i in 1...8 {
                Thread.sleep(forTimeInterval: 3)
                if self.isDaemonAlive() {
                    DispatchQueue.main.async {
                        self.isRunning = true
                        self.statusMessage = nil
                    }
                    self.fetchContainers()
                    self.fetchVersion()
                    return
                }
                DispatchQueue.main.async {
                    self.statusMessage = "VM booting... (\(i * 3)s)"
                }
            }
            DispatchQueue.main.async {
                self.statusMessage = "Started but VM may still be booting"
            }
            self.poll()
        }
    }

    func stopMako() {
        guard let path = findMakoBinary() else {
            DispatchQueue.main.async {
                self.statusMessage = "Cannot find mako binary"
            }
            return
        }

        DispatchQueue.main.async {
            self.statusMessage = "Stopping Mako..."
        }

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            let process = Process()
            process.executableURL = URL(fileURLWithPath: path)
            process.arguments = ["stop"]
            do {
                try process.run()
                process.waitUntilExit()
            } catch {
                DispatchQueue.main.async {
                    self.statusMessage = "Stop error: \(error.localizedDescription)"
                }
                return
            }
            DispatchQueue.main.async {
                self.isRunning = false
                self.containers = []
                self.statusMessage = nil
            }
        }
    }

    func stopContainer(_ id: String) {
        dockerAPIPost(path: "/containers/\(id)/stop")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            self?.fetchContainers()
        }
    }

    func startContainer(_ id: String) {
        dockerAPIPost(path: "/containers/\(id)/start")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            self?.fetchContainers()
        }
    }

    func removeContainer(_ id: String) {
        dockerAPIDelete(path: "/containers/\(id)?force=true")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            self?.fetchContainers()
        }
    }

    // MARK: - Private

    private func isDaemonAlive() -> Bool {
        // Check PID file first
        let pidFile = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".mako/makod.pid").path
        if FileManager.default.fileExists(atPath: pidFile),
           let pidStr = try? String(contentsOfFile: pidFile, encoding: .utf8),
           let pid = Int32(pidStr.trimmingCharacters(in: .whitespacesAndNewlines)) {
            return kill(pid, 0) == 0
        }

        // Fallback: check if the Docker socket exists and responds
        if FileManager.default.fileExists(atPath: socketPath),
           dockerAPIGet(path: "/version") != nil {
            return true
        }

        // Fallback: pgrep
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
        proc.arguments = ["-f", "makod"]
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = FileHandle.nullDevice
        do {
            try proc.run()
            proc.waitUntilExit()
            return proc.terminationStatus == 0
        } catch {
            return false
        }
    }

    private func fetchContainers() {
        guard let data = dockerAPIGet(path: "/containers/json?all=true") else { return }
        if let parsed = try? JSONDecoder().decode([ContainerInfo].self, from: data) {
            DispatchQueue.main.async { self.containers = parsed }
        }
    }

    private func fetchVersion() {
        guard let data = dockerAPIGet(path: "/version") else { return }
        if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let version = json["Version"] as? String {
            DispatchQueue.main.async { self.dockerVersion = version }
        }
    }

    private func dockerAPIGet(path: String) -> Data? {
        return dockerRequest(method: "GET", path: path)
    }

    private func dockerAPIPost(path: String) {
        _ = dockerRequest(method: "POST", path: path)
    }

    private func dockerAPIDelete(path: String) {
        _ = dockerRequest(method: "DELETE", path: path)
    }

    private func dockerRequest(method: String, path: String) -> Data? {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = socketPath.utf8CString
        guard pathBytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else { return nil }
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: pathBytes.count) { dest in
                for (i, byte) in pathBytes.enumerated() { dest[i] = byte }
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.connect(fd, sockPtr, addrLen)
            }
        }
        guard connectResult == 0 else { return nil }

        let request = "\(method) \(path) HTTP/1.0\r\nHost: localhost\r\n\r\n"
        _ = request.withCString { Darwin.write(fd, $0, strlen($0)) }

        var responseData = Data()
        var buf = [UInt8](repeating: 0, count: 65536)
        while true {
            let n = Darwin.read(fd, &buf, buf.count)
            if n <= 0 { break }
            responseData.append(contentsOf: buf[..<n])
        }

        guard let headerEnd = responseData.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        return responseData.subdata(in: headerEnd.upperBound..<responseData.endIndex)
    }

    private func findMakoBinary() -> String? {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        var candidates = [
            "/usr/local/bin/mako",
            "\(home)/.cargo/bin/mako",
            Bundle.main.bundlePath + "/../mako",
        ]

        // Also check common dev build locations relative to the GUI binary
        if let execPath = Bundle.main.executablePath {
            let execDir = (execPath as NSString).deletingLastPathComponent
            // Traverse up from .build/release/ to find target/release/mako
            var dir = execDir
            for _ in 0..<8 {
                let candidate = "\(dir)/target/release/mako"
                candidates.append(candidate)
                dir = (dir as NSString).deletingLastPathComponent
            }
        }

        // Also check the project directory directly
        candidates.append("\(home)/dev/docker-clone/target/release/mako")

        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }
}
