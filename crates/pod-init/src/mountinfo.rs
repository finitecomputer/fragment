//! Is the mount point mounted from the mapper device? Answered from
//! /proc/self/mountinfo by device number, so it does not depend on how
//! the kernel spells the source (/dev/mapper/<name> or /dev/dm-<n>).

use std::path::Path;

/// Linux's dev_t encoding (glibc's gnu_dev_major and gnu_dev_minor).
pub fn split_dev(dev: u64) -> (u64, u64) {
    let major = ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff);
    let minor = ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff);
    (major, minor)
}

/// mountinfo escapes space, tab, newline and backslash as \ooo octal.
fn unescape(field: &str) -> String {
    let b = field.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b.get(i + 1..i + 4).and_then(|o| std::str::from_utf8(o).ok()).and_then(|o| u8::from_str_radix(o, 8).ok());
        match (b[i], octal) {
            (b'\\', Some(c)) => {
                out.push(c);
                i += 4;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether any line of `mountinfo` mounts device `dev` at `mount`.
pub fn mounted_from(mountinfo: &str, mount: &Path, dev: (u64, u64)) -> bool {
    let want = format!("{}:{}", dev.0, dev.1);
    mountinfo.lines().any(|line| {
        let fields: Vec<&str> = line.split(' ').collect();
        match (fields.get(2), fields.get(4)) {
            (Some(d), Some(m)) => *d == want && Path::new(&unescape(m)) == mount,
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
22 1 252:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
36 22 253:0 / /srv/pod rw,relatime shared:9 - ext4 /dev/mapper/pod rw,discard
37 22 253:1 / /srv/my\\040pod rw - ext4 /dev/dm-1 rw";

    #[test]
    fn finds_the_mount_by_device_number() {
        assert!(mounted_from(INFO, Path::new("/srv/pod"), (253, 0)));
        assert!(!mounted_from(INFO, Path::new("/srv/pod"), (253, 1)));
        assert!(!mounted_from(INFO, Path::new("/srv"), (253, 0)));
        assert!(mounted_from(INFO, Path::new("/srv/my pod"), (253, 1)));
        assert!(!mounted_from("garbage", Path::new("/srv/pod"), (253, 0)));
    }

    #[test]
    fn splits_linux_device_numbers() {
        // makedev(253, 0) and makedev(259, 300000).
        assert_eq!(split_dev(0xfd00), (253, 0));
        let dev = (300000u64 & 0xff) | ((259 & 0xfff) << 8) | ((300000 & !0xff) << 12);
        assert_eq!(split_dev(dev), (259, 300000));
    }
}
