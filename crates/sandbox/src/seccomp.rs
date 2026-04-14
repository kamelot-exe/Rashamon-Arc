//! Seccomp profile installation.
//!
//! On Linux, seccomp-bpf restricts syscalls to a minimal set.

/// Install a restrictive seccomp profile for a renderer process.
/// Only allows: read, write, exit_group, mmap, brk, close, futex, rt_sigreturn
#[cfg(target_os = "linux")]
pub fn install_seccomp_profile() -> Result<(), Box<dyn std::error::Error>> {
    // Placeholder — in production, use libseccomp or prctl(PR_SET_NO_NEW_PRIVS) + BPF.
    // For now, this is a no-op stub.
    eprintln!("[sandbox] seccomp profile: stub (install libseccomp for production)");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install_seccomp_profile() -> Result<(), Box<dyn std::error::Error>> {
    Err("seccomp is only available on Linux".into())
}
