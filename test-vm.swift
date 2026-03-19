import Foundation
import Virtualization

let kernelPath = NSString("~/.mako/vmlinux-raw").expandingTildeInPath
let rootfsPath = NSString("~/.mako/rootfs.img").expandingTildeInPath

print("Kernel: \(kernelPath) exists=\(FileManager.default.fileExists(atPath: kernelPath))")
print("Rootfs: \(rootfsPath) exists=\(FileManager.default.fileExists(atPath: rootfsPath))")

let bootLoader = VZLinuxBootLoader(kernelURL: URL(fileURLWithPath: kernelPath))
bootLoader.commandLine = "console=hvc0 root=/dev/vda rw"

let config = VZVirtualMachineConfiguration()
config.cpuCount = 2
config.memorySize = 1 * 1024 * 1024 * 1024  // 1 GB

config.bootLoader = bootLoader

let serialPipe = Pipe()
let inputPipe = Pipe()
let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
serial.attachment = VZFileHandleSerialPortAttachment(
    fileHandleForReading: inputPipe.fileHandleForReading,
    fileHandleForWriting: serialPipe.fileHandleForWriting
)
config.serialPorts = [serial]

// Read serial output in background
serialPipe.fileHandleForReading.readabilityHandler = { handle in
    let data = handle.availableData
    if let str = String(data: data, encoding: .utf8), !str.isEmpty {
        print("[serial] \(str)", terminator: "")
    }
}

do {
    let disk = try VZDiskImageStorageDeviceAttachment(
        url: URL(fileURLWithPath: rootfsPath),
        readOnly: false
    )
    config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk)]
} catch {
    print("Disk error: \(error)")
    exit(1)
}

config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

do {
    try config.validate()
    print("Config validated OK")
} catch {
    print("Validation failed: \(error)")
    exit(1)
}

let vm = VZVirtualMachine(configuration: config)
print("Starting VM...")

vm.start { result in
    switch result {
    case .success:
        print("VM started successfully!")
    case .failure(let error):
        let nsError = error as NSError
        print("VM start FAILED:")
        print("  domain: \(nsError.domain)")
        print("  code: \(nsError.code)")
        print("  description: \(nsError.localizedDescription)")
        print("  userInfo: \(nsError.userInfo)")
        exit(1)
    }
}

CFRunLoopRun()
