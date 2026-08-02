use std::collections::{HashMap, HashSet};

/// Memory the renderer processes of Atoll use, in bytes. Shared pages are counted in proportion
/// to the processes sharing them. `None` where it cannot be measured.
///
/// Renderers keep the command line of the zygote they are forked from, so they are found as the
/// processes below the sandboxed zygote. The GPU process runs below the zygote without sandbox
/// and the network service directly below Atoll, both are left out.
#[cfg(target_os = "linux")]
pub fn renderer_memory() -> Option<u64> {
    use std::fs;

    let mut parents = HashMap::new();
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(parent) = fs::read_to_string(entry.path().join("stat"))
            .ok()
            .as_deref()
            .and_then(parent_of)
        {
            parents.insert(pid, parent);
        }
    }
    let own = std::process::id();
    let zygote = parents
        .iter()
        .filter(|(_, parent)| **parent == own)
        .map(|(pid, _)| *pid)
        .find(|pid| {
            fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|command| is_sandboxed_zygote(&command))
        });
    let total = descendants(&parents, zygote.unwrap_or(own))
        .into_iter()
        .filter_map(|pid| {
            fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .ok()
                .as_deref()
                .and_then(proportional_size)
        })
        .sum();
    Some(total)
}

#[cfg(not(target_os = "linux"))]
pub fn renderer_memory() -> Option<u64> {
    None
}

/// The parent process id from the content of `/proc/<pid>/stat`. The process name may contain
/// spaces and parentheses, so the fields are counted after its closing parenthesis.
fn parent_of(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// The proportional set size in bytes from the content of `/proc/<pid>/smaps_rollup`.
fn proportional_size(rollup: &str) -> Option<u64> {
    let line = rollup.lines().find(|line| line.starts_with("Pss:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

fn is_sandboxed_zygote(command_line: &[u8]) -> bool {
    let mut arguments = command_line.split(|byte| *byte == 0);
    let zygote = arguments
        .clone()
        .any(|argument| argument == b"--type=zygote");
    zygote && !arguments.any(|argument| argument == b"--no-zygote-sandbox")
}

/// Every process below `root`.
fn descendants(parents: &HashMap<u32, u32>, root: u32) -> HashSet<u32> {
    let mut found = HashSet::new();
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        for (child, _) in parents
            .iter()
            .filter(|(_, candidate)| **candidate == parent)
        {
            if found.insert(*child) {
                pending.push(*child);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_follows_the_process_name() {
        assert_eq!(parent_of("1234 (atoll) S 1200 1234 1234 0 -1"), Some(1200));
        assert_eq!(
            parent_of("1300 (Web Content (x)) R 1234 1300 1234 0 -1"),
            Some(1234)
        );
        assert_eq!(parent_of("garbage"), None);
    }

    #[test]
    fn the_proportional_size_is_read_in_bytes() {
        let rollup = "55d0c8a4c000-7ffd8e7f6000 ---p 00000000 00:00 0 [rollup]\n\
                      Rss:              204800 kB\n\
                      Pss:              150016 kB\n\
                      Pss_Anon:         120000 kB\n";
        assert_eq!(proportional_size(rollup), Some(150_016 * 1024));
        assert_eq!(proportional_size("Rss: 1 kB"), None);
    }

    #[test]
    fn renderers_hang_below_the_sandboxed_zygote() {
        assert!(is_sandboxed_zygote(
            b"/opt/atoll\0--type=zygote\0--lang=de\0"
        ));
        assert!(!is_sandboxed_zygote(
            b"/opt/atoll\0--type=zygote\0--no-zygote-sandbox\0"
        ));
        assert!(!is_sandboxed_zygote(b"/opt/atoll\0--type=utility\0"));
        assert!(!is_sandboxed_zygote(b"/opt/atoll\0--type=zygotes\0"));
    }

    #[test]
    fn descendants_include_grandchildren() {
        let parents = HashMap::from([(2, 1), (3, 2), (4, 3), (5, 9), (6, 1)]);
        assert_eq!(descendants(&parents, 1), HashSet::from([2, 3, 4, 6]));
        assert!(descendants(&parents, 4).is_empty());
    }
}
