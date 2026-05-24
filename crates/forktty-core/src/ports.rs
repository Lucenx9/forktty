//! Listening-TCP-port discovery for a workspace's terminal child processes.
//!
//! All functions take an explicit `proc_root` (normally `/proc`) so the parsing
//! logic can be exercised against fixture directories in tests without touching
//! the real procfs. Discovery is best-effort: unreadable or malformed entries
//! are skipped rather than propagated as errors, because a sidebar hint must
//! never break on a racing process exit.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// `/proc/net/tcp` connection state for a listening socket (`TCP_LISTEN`).
const TCP_LISTEN: &str = "0A";

/// Walk `proc_root` and return `roots` together with all of their transitive
/// descendant PIDs, derived from each process's parent PID in `stat`.
pub fn descendant_pids(roots: &[i32], proc_root: &Path) -> BTreeSet<i32> {
    let children = build_child_map(proc_root);
    let mut out = BTreeSet::new();
    let mut stack: Vec<i32> = roots.iter().copied().filter(|pid| *pid > 0).collect();
    while let Some(pid) = stack.pop() {
        if !out.insert(pid) {
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids.iter().copied());
        }
    }
    out
}

/// Local TCP ports in `LISTEN` state held by `roots` or any descendant process.
pub fn listening_ports(roots: &[i32], proc_root: &Path) -> BTreeSet<u16> {
    let pids = descendant_pids(roots, proc_root);
    if pids.is_empty() {
        return BTreeSet::new();
    }
    let inode_ports = listen_inode_ports(proc_root);
    if inode_ports.is_empty() {
        return BTreeSet::new();
    }
    let mut ports = BTreeSet::new();
    for pid in pids {
        for inode in socket_inodes_for_pid(pid, proc_root) {
            if let Some(port) = inode_ports.get(&inode) {
                ports.insert(*port);
            }
        }
    }
    ports
}

/// Map parent PID -> child PIDs by parsing every `<proc_root>/<pid>/stat`.
fn build_child_map(proc_root: &Path) -> BTreeMap<i32, Vec<i32>> {
    let mut map: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(proc_root) else {
        return map;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let stat_path = entry.path().join("stat");
        let Ok(contents) = fs::read_to_string(&stat_path) else {
            continue;
        };
        if let Some(ppid) = parse_ppid(&contents) {
            map.entry(ppid).or_default().push(pid);
        }
    }
    map
}

/// Extract the parent PID (4th field) from a `/proc/<pid>/stat` line.
///
/// The 2nd field is `(comm)` and may itself contain spaces or parentheses, so
/// the scan resumes after the final `)` to keep field offsets correct.
fn parse_ppid(stat: &str) -> Option<i32> {
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 1..)?;
    // After comm: state (field 3) then ppid (field 4).
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<i32>().ok()
}

/// Map socket inode -> local port for `LISTEN` sockets in tcp and tcp6 tables.
fn listen_inode_ports(proc_root: &Path) -> BTreeMap<u64, u16> {
    let mut map = BTreeMap::new();
    for table in ["net/tcp", "net/tcp6"] {
        let Ok(contents) = fs::read_to_string(proc_root.join(table)) else {
            continue;
        };
        for line in contents.lines().skip(1) {
            if let Some((inode, port)) = parse_listen_line(line) {
                map.insert(inode, port);
            }
        }
    }
    map
}

/// Parse one `/proc/net/tcp{,6}` row, yielding `(inode, local_port)` only when
/// the socket is in `LISTEN` state.
fn parse_listen_line(line: &str) -> Option<(u64, u16)> {
    let mut fields = line.split_whitespace();
    let _sl = fields.next()?;
    let local_address = fields.next()?;
    let _rem_address = fields.next()?;
    let state = fields.next()?;
    if state != TCP_LISTEN {
        return None;
    }
    // Skip tx_queue:rx_queue, tr:tm->when, retrnsmt, uid, timeout to reach inode.
    let inode_field = fields.nth(5)?;
    let inode = inode_field.parse::<u64>().ok()?;
    let port_hex = local_address.rsplit(':').next()?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;
    Some((inode, port))
}

/// Socket inodes referenced by `<proc_root>/<pid>/fd/*` symlinks of the form
/// `socket:[12345]`.
fn socket_inodes_for_pid(pid: i32, proc_root: &Path) -> BTreeSet<u64> {
    let mut inodes = BTreeSet::new();
    let fd_dir = proc_root.join(pid.to_string()).join("fd");
    let Ok(entries) = fs::read_dir(&fd_dir) else {
        return inodes;
    };
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(inode) = parse_socket_inode(&target.to_string_lossy()) {
            inodes.insert(inode);
        }
    }
    inodes
}

/// Extract `N` from an fd symlink target `socket:[N]`.
fn parse_socket_inode(target: &str) -> Option<u64> {
    let rest = target.strip_prefix("socket:[")?;
    let digits = rest.strip_suffix(']')?;
    digits.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Write `<proc>/<pid>/stat` with the given comm and ppid.
    fn write_stat(proc: &Path, pid: i32, comm: &str, ppid: i32) {
        let dir = proc.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        // pid (comm) state ppid pgrp ...
        fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) S {ppid} 0 0 0 0\n"),
        )
        .unwrap();
    }

    /// Create `<proc>/<pid>/fd/<n>` symlinks pointing at the given socket inodes.
    fn write_fd_sockets(proc: &Path, pid: i32, inodes: &[u64]) {
        let dir = proc.join(pid.to_string()).join("fd");
        fs::create_dir_all(&dir).unwrap();
        for (n, inode) in inodes.iter().enumerate() {
            let link = dir.join(n.to_string());
            // Target need not exist; we only read the link text.
            symlink(format!("socket:[{inode}]"), link).unwrap();
        }
    }

    fn write_net_tcp(proc: &Path, file: &str, body: &str) {
        let net = proc.join("net");
        fs::create_dir_all(&net).unwrap();
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        fs::write(net.join(file), format!("{header}{body}")).unwrap();
    }

    fn proc_dir() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        (tmp, root)
    }

    #[test]
    fn descendant_pids_collects_transitive_children() {
        let (_tmp, proc) = proc_dir();
        write_stat(&proc, 100, "bash", 1);
        write_stat(&proc, 200, "node", 100);
        write_stat(&proc, 300, "esbuild", 200);
        write_stat(&proc, 999, "unrelated", 1);

        let pids = descendant_pids(&[100], &proc);

        assert_eq!(pids, BTreeSet::from([100, 200, 300]));
    }

    #[test]
    fn parse_ppid_handles_comm_with_spaces_and_parens() {
        let stat = "4242 (weird )( name) S 4200 0 0 0 0";
        assert_eq!(parse_ppid(stat), Some(4200));
    }

    #[test]
    fn parse_listen_line_extracts_port_only_when_listening() {
        // local 0.0.0.0:1F90 (port 8080), state 0A (LISTEN), inode 56789.
        let listen = "  0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 56789 1 ffff 100";
        assert_eq!(parse_listen_line(listen), Some((56789, 8080)));

        // state 01 (ESTABLISHED) is ignored.
        let established = "  1: 00000000:1F91 01010101:0050 01 00000000:00000000 00:00000000 00000000  1000        0 56790 1 ffff 100";
        assert_eq!(parse_listen_line(established), None);
    }

    #[test]
    fn parse_socket_inode_reads_socket_links_only() {
        assert_eq!(parse_socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(parse_socket_inode("/dev/pts/3"), None);
        assert_eq!(parse_socket_inode("anon_inode:[eventfd]"), None);
    }

    #[test]
    fn listening_ports_maps_descendant_sockets_to_ports() {
        let (_tmp, proc) = proc_dir();
        // bash(100) -> node(200) holding the listening socket on inode 56789.
        write_stat(&proc, 100, "bash", 1);
        write_stat(&proc, 200, "node", 100);
        write_fd_sockets(&proc, 200, &[56789]);
        // An unrelated process holding a different listening socket.
        write_stat(&proc, 900, "other", 1);
        write_fd_sockets(&proc, 900, &[11111]);

        write_net_tcp(
            &proc,
            "tcp",
            "  0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 56789 1 ffff 100\n  1: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 ffff 100\n",
        );
        write_net_tcp(&proc, "tcp6", "");

        let ports = listening_ports(&[100], &proc);

        assert_eq!(ports, BTreeSet::from([8080]));
    }

    #[test]
    fn listening_ports_includes_tcp6_and_dedupes() {
        let (_tmp, proc) = proc_dir();
        write_stat(&proc, 100, "bash", 1);
        write_fd_sockets(&proc, 100, &[1, 2]);
        write_net_tcp(
            &proc,
            "tcp",
            "  0: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 ffff 100\n",
        );
        // tcp6 entry on inode 2 maps to the same port 3000 -> deduped to one.
        write_net_tcp(
            &proc,
            "tcp6",
            "  0: 00000000000000000000000000000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2 1 ffff 100\n",
        );

        let ports = listening_ports(&[100], &proc);

        assert_eq!(ports, BTreeSet::from([3000]));
    }

    #[test]
    fn listening_ports_empty_when_no_pids() {
        let (_tmp, proc) = proc_dir();
        assert!(listening_ports(&[], &proc).is_empty());
    }
}
