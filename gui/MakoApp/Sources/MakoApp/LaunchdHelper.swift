import Foundation
import ServiceManagement

enum LaunchdHelper {
    private static let plistName = "com.mako.daemon"

    static func installLaunchAgent(makodPath: String) throws {
        let plistContent: [String: Any] = [
            "Label": plistName,
            "ProgramArguments": [makodPath],
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
            "StandardOutPath": "\(NSHomeDirectory())/.mako/makod.stdout.log",
            "StandardErrorPath": "\(NSHomeDirectory())/.mako/makod.stderr.log",
        ]

        let launchAgentsDir = "\(NSHomeDirectory())/Library/LaunchAgents"
        try FileManager.default.createDirectory(atPath: launchAgentsDir, withIntermediateDirectories: true)

        let plistPath = "\(launchAgentsDir)/\(plistName).plist"
        let data = try PropertyListSerialization.data(fromPropertyList: plistContent, format: .xml, options: 0)
        try data.write(to: URL(fileURLWithPath: plistPath))

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = ["load", plistPath]
        try process.run()
        process.waitUntilExit()
    }

    static func uninstallLaunchAgent() throws {
        let plistPath = "\(NSHomeDirectory())/Library/LaunchAgents/\(plistName).plist"
        if FileManager.default.fileExists(atPath: plistPath) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            process.arguments = ["unload", plistPath]
            try process.run()
            process.waitUntilExit()
            try FileManager.default.removeItem(atPath: plistPath)
        }
    }

    static var isInstalled: Bool {
        FileManager.default.fileExists(
            atPath: "\(NSHomeDirectory())/Library/LaunchAgents/\(plistName).plist"
        )
    }
}
