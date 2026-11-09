echo "== id"; id
echo "== os"; head -3 /etc/os-release; uname -r
echo "== /dev/fuse"; ls -la /dev/fuse 2>&1
echo "== filesystems"; grep -i fuse /proc/filesystems
echo "== caps of exec (self)"; grep -E '^Cap(Inh|Prm|Eff|Bnd|Amb)' /proc/self/status
echo "== caps of pid1"; grep -E '^Cap(Prm|Eff|Bnd)' /proc/1/status; cat /proc/1/cmdline | tr '\0' ' '; echo
echo "== seccomp/nnp"; grep -E 'Seccomp|NoNewPrivs' /proc/self/status
echo "== tools"; for b in s3fs fusermount fusermount3 mount.fuse3 capsh apt-get python3 sandbox-shim; do printf '%s: ' $b; command -v $b || echo none; done
echo "== mounts"; awk '{print $2, $3}' /proc/self/mounts | head -40
echo "== df"; df -h / /data /tmp 2>&1
echo "== mem/cpu"; nproc; free -m | head -2
