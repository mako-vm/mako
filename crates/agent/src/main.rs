use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const VSOCK_PORT: u32 = 2375;
const DOCKER_SOCK: &str = "/var/run/docker.sock";
const HOST_CID: u32 = 2;
const NUM_WORKERS: usize = 8;

fn main() {
    eprintln!("mako-agent: reverse vsock relay, {NUM_WORKERS} workers");
    eprintln!("mako-agent: host CID={HOST_CID} port={VSOCK_PORT} -> {DOCKER_SOCK}");

    for i in 0..NUM_WORKERS {
        std::thread::spawn(move || worker_loop(i));
    }

    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn worker_loop(id: usize) {
    loop {
        match vsock_connect(HOST_CID, VSOCK_PORT) {
            Ok(fd) => {
                eprintln!("mako-agent[{id}]: connected to host (fd={fd})");
                if let Err(e) = handle_connection(fd) {
                    eprintln!("mako-agent[{id}]: relay error: {e}");
                }
                eprintln!("mako-agent[{id}]: relay done, reconnecting");
            }
            Err(e) => {
                eprintln!("mako-agent[{id}]: connect failed: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn handle_connection(vsock_fd: RawFd) -> io::Result<()> {
    let mut vsock = unsafe { std::fs::File::from_raw_fd(vsock_fd) };

    // Block until host sends data (Docker client connected and sent request)
    let mut first_buf = [0u8; 65536];
    let first_n = vsock.read(&mut first_buf)?;
    if first_n == 0 {
        return Ok(());
    }
    eprintln!("mako-agent: received {first_n} bytes from host");

    // Connect to dockerd with retries
    let mut docker = None;
    for attempt in 0..5 {
        match UnixStream::connect(DOCKER_SOCK) {
            Ok(s) => {
                docker = Some(s);
                break;
            }
            Err(e) => {
                eprintln!("mako-agent: dockerd connect attempt {attempt}: {e}");
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
    let mut docker = docker.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "dockerd unavailable after retries",
        )
    })?;
    docker.write_all(&first_buf[..first_n])?;
    eprintln!("mako-agent: forwarded {first_n} bytes to dockerd");

    let vsock_w = vsock.try_clone()?;
    let docker_r = docker.try_clone()?;

    let vsock_fd_for_shutdown = vsock_fd;

    // vsock -> docker (continue forwarding)
    let mut vr = vsock;
    let mut dw = docker;
    let v2d = std::thread::spawn(move || -> io::Result<()> {
        let mut buf = [0u8; 65536];
        loop {
            let n = vr.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dw.write_all(&buf[..n])?;
        }
        dw.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    });

    // docker -> vsock
    let mut dr = docker_r;
    let mut vw = vsock_w;
    let d2v = std::thread::spawn(move || -> io::Result<()> {
        let mut buf = [0u8; 65536];
        loop {
            let n = dr.read(&mut buf)?;
            if n == 0 {
                break;
            }
            vw.write_all(&buf[..n])?;
        }
        unsafe {
            libc::shutdown(vsock_fd_for_shutdown, libc::SHUT_WR);
        }
        Ok(())
    });

    let _ = v2d.join();
    let _ = d2v.join();
    Ok(())
}

fn vsock_connect(cid: u32, port: u32) -> io::Result<RawFd> {
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    addr.svm_family = libc::AF_VSOCK as _;
    addr.svm_cid = cid;
    addr.svm_port = port;

    let ret = unsafe {
        libc::connect(
            fd,
            &addr as *const libc::sockaddr_vm as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_vm>() as u32,
        )
    };
    if ret < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }

    Ok(fd)
}
