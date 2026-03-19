#ifndef MAKO_FFI_H
#define MAKO_FFI_H

#include <stdint.h>
#include <stdbool.h>

typedef void MakoVMHandle;
typedef void (*mako_vm_callback)(bool success, const char* error_msg);

MakoVMHandle* mako_vm_create(
    int32_t cpu_count,
    uint64_t memory_bytes,
    const char* kernel_path,
    const char* initrd_path,   /* nullable */
    const char* rootfs_path,
    bool rosetta,
    uint32_t vsock_control_port,
    uint32_t vsock_docker_port
);

int32_t mako_vm_configure(MakoVMHandle* handle);
void mako_vm_start(MakoVMHandle* handle, mako_vm_callback callback);
void mako_vm_stop(MakoVMHandle* handle, mako_vm_callback callback);
bool mako_vm_is_running(MakoVMHandle* handle);
const char* mako_vm_get_state(MakoVMHandle* handle);
const char* mako_vm_get_error(MakoVMHandle* handle);

/* Returns fd for reading VM serial console output. */
int32_t mako_vm_get_serial_read_fd(MakoVMHandle* handle);

/*
 * Connect to a vsock port in the guest VM.
 * On success, writes read/write fds to out_read_fd and out_write_fd and returns 0.
 * On failure, returns -1.
 */
int32_t mako_vm_vsock_connect(
    MakoVMHandle* handle,
    uint32_t port,
    int32_t* out_read_fd,
    int32_t* out_write_fd
);

/*
 * Start listening for guest-initiated vsock connections on the given port.
 * Each incoming connection from the guest will be queued and can be retrieved
 * by calling mako_vm_vsock_accept (blocks until one is available).
 * Returns 0 on success, -1 on failure.
 */
int32_t mako_vm_vsock_listen(MakoVMHandle* handle, uint32_t port);

/*
 * Accept the next guest-initiated vsock connection (blocks until available).
 * Returns 0 on success (fd written to out_fd), -1 on failure.
 */
int32_t mako_vm_vsock_accept(MakoVMHandle* handle, uint32_t port, int32_t* out_fd);

void mako_vm_destroy(MakoVMHandle* handle);

#endif
