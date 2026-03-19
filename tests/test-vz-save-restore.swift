#!/usr/bin/env swift
// Minimal test for VZVirtualMachine save/restore on this macOS version.
// Usage: swift test-vz-save-restore.swift <kernel> <rootfs>

import Foundation
import Virtualization

guard CommandLine.arguments.count == 3 else {
    print("Usage: \(CommandLine.arguments[0]) <kernel-path> <rootfs-path>")
    exit(1)
}

let kernelPath = CommandLine.arguments[1]
let rootfsPath = CommandLine.arguments[2]
let statePath = "/tmp/mako-test-vm-state"

print("Testing VZ save/restore...")
print("  Kernel: \(kernelPath)")
print("  Rootfs: \(rootfsPath)")
print("  macOS: \(ProcessInfo.processInfo.operatingSystemVersionString)")

// Create configuration
let bootLoader = VZLinuxBootLoader(kernelURL: URL(fileURLWithPath: kernelPath))
bootLoader.commandLine = "console=hvc0 root=/dev/vda rw"

let config = VZVirtualMachineConfiguration()
config.platform = VZGenericPlatformConfiguration()
config.cpuCount = 2
config.memorySize = 512 * 1024 * 1024
config.bootLoader = bootLoader

let serialPort = VZVirtioConsoleDeviceSerialPortConfiguration()
let pipe = Pipe()
let inputPipe = Pipe()
serialPort.attachment = VZFileHandleSerialPortAttachment(
    fileHandleForReading: inputPipe.fileHandleForReading,
    fileHandleForWriting: pipe.fileHandleForWriting
)
config.serialPorts = [serialPort]

do {
    let disk = try VZDiskImageStorageDeviceAttachment(url: URL(fileURLWithPath: rootfsPath), readOnly: false)
    config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk)]
} catch {
    print("ERROR: disk attachment failed: \(error)")
    exit(1)
}

config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
config.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
config.socketDevices = [VZVirtioSocketDeviceConfiguration()]

let net = VZVirtioNetworkDeviceConfiguration()
net.attachment = VZNATNetworkDeviceAttachment()
config.networkDevices = [net]

do {
    try config.validate()
    print("  Config validated OK")
} catch {
    print("ERROR: config validation failed: \(error)")
    exit(1)
}

// Create VM
let vm = VZVirtualMachine(configuration: config)
print("  VM created, state=\(vm.state.rawValue)")

// Start VM
let startSem = DispatchSemaphore(value: 0)
var startErr: Error?
vm.start { result in
    switch result {
    case .success: break
    case .failure(let e): startErr = e
    }
    startSem.signal()
}
startSem.wait()
if let err = startErr {
    print("ERROR: start failed: \(err)")
    exit(1)
}
print("  VM started, state=\(vm.state.rawValue)")

// Wait a bit for the kernel to boot
print("  Waiting 5 seconds for kernel to boot...")
Thread.sleep(forTimeInterval: 5.0)
print("  VM state after boot: \(vm.state.rawValue)")

// Pause
let pauseSem = DispatchSemaphore(value: 0)
var pauseErr: Error?
vm.pause { result in
    switch result {
    case .success: break
    case .failure(let e): pauseErr = e
    }
    pauseSem.signal()
}
pauseSem.wait()
if let err = pauseErr {
    print("ERROR: pause failed: \(err)")
    exit(1)
}
print("  VM paused, state=\(vm.state.rawValue)")

// Save state
let saveSem = DispatchSemaphore(value: 0)
var saveErr: Error?
let saveURL = URL(fileURLWithPath: statePath)
vm.saveMachineStateTo(url: saveURL) { error in
    saveErr = error
    saveSem.signal()
}
saveSem.wait()
if let err = saveErr {
    let nsErr = err as NSError
    print("ERROR: save failed: domain=\(nsErr.domain) code=\(nsErr.code) \(nsErr.localizedDescription)")
    print("  userInfo: \(nsErr.userInfo)")
    exit(1)
}
print("  State saved, state=\(vm.state.rawValue)")

// Force stop the VM
let stopSem = DispatchSemaphore(value: 0)
var stopErr: Error?
vm.stop { error in
    stopErr = error
    stopSem.signal()
}
stopSem.wait()
if let err = stopErr {
    print("WARN: stop failed (non-fatal): \(err)")
}
print("  VM stopped, state=\(vm.state.rawValue)")

// Wait for VM to fully stop
Thread.sleep(forTimeInterval: 1.0)
print("  VM state after stop: \(vm.state.rawValue)")

// Now create a NEW VM with same config and try to restore
print("\n--- Creating new VM for restore ---")

let config2 = VZVirtualMachineConfiguration()
config2.platform = VZGenericPlatformConfiguration()
config2.cpuCount = 2
config2.memorySize = 512 * 1024 * 1024
config2.bootLoader = bootLoader

let serialPort2 = VZVirtioConsoleDeviceSerialPortConfiguration()
let pipe2 = Pipe()
let inputPipe2 = Pipe()
serialPort2.attachment = VZFileHandleSerialPortAttachment(
    fileHandleForReading: inputPipe2.fileHandleForReading,
    fileHandleForWriting: pipe2.fileHandleForWriting
)
config2.serialPorts = [serialPort2]

do {
    let disk2 = try VZDiskImageStorageDeviceAttachment(url: URL(fileURLWithPath: rootfsPath), readOnly: false)
    config2.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: disk2)]
} catch {
    print("ERROR: disk2 attachment failed: \(error)")
    exit(1)
}

config2.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
config2.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]
config2.socketDevices = [VZVirtioSocketDeviceConfiguration()]

let net2 = VZVirtioNetworkDeviceConfiguration()
net2.attachment = VZNATNetworkDeviceAttachment()
config2.networkDevices = [net2]

do {
    try config2.validate()
    print("  Config2 validated OK")
} catch {
    print("ERROR: config2 validation failed: \(error)")
    exit(1)
}

let vm2 = VZVirtualMachine(configuration: config2)
print("  VM2 created, state=\(vm2.state.rawValue)")

// Restore
let restoreSem = DispatchSemaphore(value: 0)
var restoreErr: Error?
vm2.restoreMachineStateFrom(url: saveURL) { error in
    restoreErr = error
    restoreSem.signal()
}
restoreSem.wait()
if let err = restoreErr {
    let nsErr = err as NSError
    print("ERROR: restore failed: domain=\(nsErr.domain) code=\(nsErr.code) \(nsErr.localizedDescription)")
    print("  userInfo: \(nsErr.userInfo)")
    exit(1)
}
print("  Restore succeeded! state=\(vm2.state.rawValue)")

// Resume
let resumeSem = DispatchSemaphore(value: 0)
var resumeErr: Error?
vm2.resume { result in
    switch result {
    case .success: break
    case .failure(let e): resumeErr = e
    }
    resumeSem.signal()
}
resumeSem.wait()
if let err = resumeErr {
    print("ERROR: resume failed: \(err)")
    exit(1)
}
print("  VM2 resumed! state=\(vm2.state.rawValue)")
print("\n*** SAVE/RESTORE WORKS! ***")

// Clean up
try? FileManager.default.removeItem(atPath: statePath)
exit(0)
