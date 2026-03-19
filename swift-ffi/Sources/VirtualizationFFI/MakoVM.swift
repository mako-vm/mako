import Foundation
import Virtualization

// MARK: - VM Wrapper

class MakoVMWrapper {
    var vm: VZVirtualMachine?
    var vmConfig: VZVirtualMachineConfiguration?
    let cpuCount: Int
    let memoryBytes: UInt64
    let kernelPath: String
    let initrdPath: String?
    let rootfsPath: String
    let rosetta: Bool
    let vsockControlPort: UInt32
    let vsockDockerPort: UInt32
    var lastError: String?
    /// Active vsock connections -- kept alive so the fds remain valid
    var vsockConnections: [VZVirtioSocketConnection] = []
    /// Queues for guest-initiated vsock connections (per port)
    var vsockAcceptQueues: [UInt32: VsockAcceptQueue] = [:]

    /// Pipe for capturing serial console output (VM stdout)
    let serialPipe = Pipe()
    /// Pipe for serial input (unused, but VZ requires a valid fd)
    let serialInputPipe = Pipe()

    init(
        cpuCount: Int,
        memoryBytes: UInt64,
        kernelPath: String,
        initrdPath: String?,
        rootfsPath: String,
        rosetta: Bool,
        vsockControlPort: UInt32,
        vsockDockerPort: UInt32
    ) {
        self.cpuCount = cpuCount
        self.memoryBytes = memoryBytes
        self.kernelPath = kernelPath
        self.initrdPath = initrdPath
        self.rootfsPath = rootfsPath
        self.rosetta = rosetta
        self.vsockControlPort = vsockControlPort
        self.vsockDockerPort = vsockDockerPort
    }

    func configure() -> Bool {
        let bootLoader = VZLinuxBootLoader(
            kernelURL: URL(fileURLWithPath: kernelPath)
        )
        bootLoader.commandLine = "console=hvc0 root=/dev/vda rw"

        if let initrd = initrdPath {
            let initrdURL = URL(fileURLWithPath: initrd)
            if FileManager.default.fileExists(atPath: initrd) {
                bootLoader.initialRamdiskURL = initrdURL
            }
        }

        let config = VZVirtualMachineConfiguration()
        config.cpuCount = cpuCount
        config.memorySize = memoryBytes
        config.bootLoader = bootLoader

        // Serial console captured to a pipe (Rust reads from the pipe fd)
        let serialPort = VZVirtioConsoleDeviceSerialPortConfiguration()
        serialPort.attachment = VZFileHandleSerialPortAttachment(
            fileHandleForReading: serialInputPipe.fileHandleForReading,
            fileHandleForWriting: serialPipe.fileHandleForWriting
        )
        config.serialPorts = [serialPort]

        // Root filesystem block device
        do {
            let diskURL = URL(fileURLWithPath: rootfsPath)
            let diskAttachment = try VZDiskImageStorageDeviceAttachment(
                url: diskURL,
                readOnly: false
            )
            config.storageDevices = [
                VZVirtioBlockDeviceConfiguration(attachment: diskAttachment)
            ]
        } catch {
            lastError = "Failed to attach disk: \(error.localizedDescription)"
            return false
        }

        // Entropy device
        config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

        // Memory balloon for dynamic memory management
        config.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]

        // vsock device for host-guest communication
        let vsockDevice = VZVirtioSocketDeviceConfiguration()
        config.socketDevices = [vsockDevice]

        // NAT network
        let networkDevice = VZVirtioNetworkDeviceConfiguration()
        networkDevice.attachment = VZNATNetworkDeviceAttachment()
        config.networkDevices = [networkDevice]

        // VirtioFS: share macOS home directory into the VM
        let homeDir = FileManager.default.homeDirectoryForCurrentUser
        let share = VZMultipleDirectoryShare(
            directories: [
                "home": VZSharedDirectory(url: homeDir, readOnly: false)
            ]
        )
        let fsConfig = VZVirtioFileSystemDeviceConfiguration(tag: "mako-share")
        fsConfig.share = share
        config.directorySharingDevices = [fsConfig]

        // Rosetta for x86 emulation on Apple Silicon
        #if arch(arm64)
        if rosetta {
            if #available(macOS 13.0, *),
               VZLinuxRosettaDirectoryShare.availability == .installed {
                do {
                    let rosettaShare = try VZLinuxRosettaDirectoryShare()
                    let rosettaConfig = VZVirtioFileSystemDeviceConfiguration(tag: "rosetta")
                    rosettaConfig.share = rosettaShare
                    config.directorySharingDevices.append(rosettaConfig)
                } catch {
                    lastError = "Rosetta setup failed (non-fatal): \(error.localizedDescription)"
                }
            }
        }
        #endif

        do {
            try config.validate()
        } catch {
            lastError = "VM configuration validation failed: \(error.localizedDescription)"
            return false
        }

        self.vmConfig = config
        return true
    }

    func createAndStart(completion: @escaping (Bool, String?) -> Void) {
        guard let config = self.vmConfig else {
            completion(false, "VM not configured")
            return
        }

        self.vm = VZVirtualMachine(configuration: config)

        guard let vm = self.vm else {
            completion(false, "Failed to create VZVirtualMachine")
            return
        }

        vm.start { result in
            switch result {
            case .success:
                completion(true, nil)
            case .failure(let error):
                let nsError = error as NSError
                let detail = "domain=\(nsError.domain) code=\(nsError.code) \(nsError.localizedDescription)"
                    + (nsError.userInfo.isEmpty ? "" : " userInfo=\(nsError.userInfo)")
                completion(false, detail)
            }
        }
    }

    func stop(completion: @escaping (Bool, String?) -> Void) {
        guard let vm = self.vm else {
            completion(false, "VM not created")
            return
        }

        do {
            try vm.requestStop()
            completion(true, nil)
        } catch {
            completion(false, error.localizedDescription)
        }
    }

    var isRunning: Bool {
        vm?.state == .running
    }

    var stateString: String {
        guard let vm = self.vm else { return "not_created" }
        switch vm.state {
        case .stopped: return "stopped"
        case .running: return "running"
        case .paused: return "paused"
        case .error: return "error"
        case .starting: return "starting"
        case .pausing: return "pausing"
        case .resuming: return "resuming"
        case .stopping: return "stopping"
        case .saving: return "saving"
        case .restoring: return "restoring"
        @unknown default: return "unknown"
        }
    }
}

// MARK: - C-callable FFI functions

@_cdecl("mako_vm_create")
func makoVMCreate(
    cpuCount: Int32,
    memoryBytes: UInt64,
    kernelPath: UnsafePointer<CChar>,
    initrdPath: UnsafePointer<CChar>?,
    rootfsPath: UnsafePointer<CChar>,
    rosetta: Bool,
    vsockControlPort: UInt32,
    vsockDockerPort: UInt32
) -> UnsafeMutableRawPointer {
    let initrd: String? = initrdPath.map { String(cString: $0) }
    let wrapper = MakoVMWrapper(
        cpuCount: Int(cpuCount),
        memoryBytes: memoryBytes,
        kernelPath: String(cString: kernelPath),
        initrdPath: initrd,
        rootfsPath: String(cString: rootfsPath),
        rosetta: rosetta,
        vsockControlPort: vsockControlPort,
        vsockDockerPort: vsockDockerPort
    )
    return Unmanaged.passRetained(wrapper).toOpaque()
}

@_cdecl("mako_vm_configure")
func makoVMConfigure(handle: UnsafeMutableRawPointer) -> Int32 {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    var result: Int32 = -1
    if Thread.isMainThread {
        result = wrapper.configure() ? 0 : -1
    } else {
        DispatchQueue.main.sync {
            result = wrapper.configure() ? 0 : -1
        }
    }
    return result
}

@_cdecl("mako_vm_start")
func makoVMStart(
    handle: UnsafeMutableRawPointer,
    callback: @convention(c) (Bool, UnsafePointer<CChar>?) -> Void
) {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    let startBlock = {
        wrapper.createAndStart { success, errorMsg in
            if let msg = errorMsg {
                msg.withCString { ptr in callback(success, ptr) }
            } else {
                callback(success, nil)
            }
        }
    }
    if Thread.isMainThread {
        startBlock()
    } else {
        DispatchQueue.main.async { startBlock() }
    }
}

@_cdecl("mako_vm_stop")
func makoVMStop(
    handle: UnsafeMutableRawPointer,
    callback: @convention(c) (Bool, UnsafePointer<CChar>?) -> Void
) {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    let stopBlock = {
        wrapper.stop { success, errorMsg in
            if let msg = errorMsg {
                msg.withCString { ptr in callback(success, ptr) }
            } else {
                callback(success, nil)
            }
        }
    }
    if Thread.isMainThread {
        stopBlock()
    } else {
        DispatchQueue.main.async { stopBlock() }
    }
}

@_cdecl("mako_vm_is_running")
func makoVMIsRunning(handle: UnsafeMutableRawPointer) -> Bool {
    Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue().isRunning
}

@_cdecl("mako_vm_get_state")
func makoVMGetState(handle: UnsafeMutableRawPointer) -> UnsafePointer<CChar>? {
    let state = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue().stateString
    return (state as NSString).utf8String
}

@_cdecl("mako_vm_get_error")
func makoVMGetError(handle: UnsafeMutableRawPointer) -> UnsafePointer<CChar>? {
    guard let error = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue().lastError else {
        return nil
    }
    return (error as NSString).utf8String
}

/// Returns the file descriptor for reading serial console output from the VM.
/// Rust can read from this fd to capture kernel/init messages (including MAKO_VM_IP).
@_cdecl("mako_vm_get_serial_read_fd")
func makoVMGetSerialReadFD(handle: UnsafeMutableRawPointer) -> Int32 {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    return wrapper.serialPipe.fileHandleForReading.fileDescriptor
}

/// Connect to a vsock port in the guest VM. Returns 0 on success, -1 on failure.
/// The connection object is retained to keep the fd alive. Call mako_vm_vsock_close to release.
@_cdecl("mako_vm_vsock_connect")
func makoVMVsockConnect(
    handle: UnsafeMutableRawPointer,
    port: UInt32,
    outReadFD: UnsafeMutablePointer<Int32>,
    outWriteFD: UnsafeMutablePointer<Int32>
) -> Int32 {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    guard let vm = wrapper.vm else {
        return -1
    }

    guard let socketDevice = vm.socketDevices.first as? VZVirtioSocketDevice else {
        return -1
    }

    let semaphore = DispatchSemaphore(value: 0)
    var success = false

    let connectBlock = {
        socketDevice.connect(toPort: port) { result in
            switch result {
            case .success(let conn):
                let fd = conn.fileDescriptor
                outReadFD.pointee = fd
                outWriteFD.pointee = fd
                // Retain the connection so the fd stays valid
                wrapper.vsockConnections.append(conn)
                success = true
            case .failure:
                success = false
            }
            semaphore.signal()
        }
    }

    if Thread.isMainThread {
        connectBlock()
    } else {
        DispatchQueue.main.async { connectBlock() }
    }

    semaphore.wait()
    return success ? 0 : -1
}

// MARK: - Vsock Accept Queue (guest-initiated connections)

class VsockAcceptQueue: NSObject, VZVirtioSocketListenerDelegate {
    private let semaphore = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var pending: [VZVirtioSocketConnection] = []

    func listener(
        _ listener: VZVirtioSocketListener,
        shouldAcceptNewConnection connection: VZVirtioSocketConnection,
        from socketDevice: VZVirtioSocketDevice
    ) -> Bool {
        lock.lock()
        pending.append(connection)
        lock.unlock()
        semaphore.signal()
        return true
    }

    /// Blocks until a connection is available, then returns it.
    func accept() -> VZVirtioSocketConnection {
        semaphore.wait()
        lock.lock()
        let conn = pending.removeFirst()
        lock.unlock()
        return conn
    }
}

@_cdecl("mako_vm_vsock_listen")
func makoVMVsockListen(
    handle: UnsafeMutableRawPointer,
    port: UInt32
) -> Int32 {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    guard let vm = wrapper.vm else { return -1 }
    guard let socketDevice = vm.socketDevices.first as? VZVirtioSocketDevice else { return -1 }

    let queue = VsockAcceptQueue()
    let listener = VZVirtioSocketListener()
    listener.delegate = queue
    wrapper.vsockAcceptQueues[port] = queue

    let setupBlock = {
        socketDevice.setSocketListener(listener, forPort: port)
    }
    if Thread.isMainThread {
        setupBlock()
    } else {
        DispatchQueue.main.sync { setupBlock() }
    }
    return 0
}

@_cdecl("mako_vm_vsock_accept")
func makoVMVsockAccept(
    handle: UnsafeMutableRawPointer,
    port: UInt32,
    outFD: UnsafeMutablePointer<Int32>
) -> Int32 {
    let wrapper = Unmanaged<MakoVMWrapper>.fromOpaque(handle).takeUnretainedValue()
    guard let queue = wrapper.vsockAcceptQueues[port] else { return -1 }

    let conn = queue.accept()
    let fd = conn.fileDescriptor
    if fd < 0 { return -1 }
    outFD.pointee = fd
    wrapper.vsockConnections.append(conn)
    return 0
}

@_cdecl("mako_vm_destroy")
func makoVMDestroy(handle: UnsafeMutableRawPointer) {
    Unmanaged<MakoVMWrapper>.fromOpaque(handle).release()
}
