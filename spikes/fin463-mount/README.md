# FIN-463 spike: mounting a staged agent read-only (never merged)

Can a computer read an old agent's staged folder from R2, read-only,
outside `/data`, without copying it in? Run on preview (account
5ae8672e) as the branch deployment `fin463-spike`, 2026-11-09, with
synthetic data only. The deployment, its bucket and its container
application are deleted.

## What is on this branch

- `cell/spike-fin463.mjs`, wired in `cell/entry.mjs`: routes
  `/api/test/spike/*`, reachable only with the branch's test secret.
  - A Computer RPC `spike(op)`: exec in the container; `mount`,
    `inspect` and `unmount` through the Sandbox SDK's `S3Mount`
    (sandbox-shim runs s3fs over FUSE); `store`, which arms the fallback.
  - `SpikeS3Gateway`: the S3 subset s3fs reads (ListObjects V1/V2, HEAD,
    ranged GET), served from the R2 binding. It applies the SDK
    gateway's policy: one bucket, one key prefix, reads only.
    Preview's deploy token can't mint an R2 S3 key, so the SDK's own
    signing `S3Gateway` was not run.
  - `SpikeStore`: the fallback. One prefix, served by HTTP Range at an
    intercepted host.
- `drive.py`: signs in an e2e person, makes and wakes their computer,
  and pulls the spike levers.
- Run in the container: `probe.sh`, `seed.py` (10,000 small files, one
  1.5 GiB blob, a 1.6 GB zip of both), `mount_bench.py` and
  `range_bench.py`.

s3fs was installed at run time (`apt-get install s3fs`, 5.7 s). For
real, it belongs in the image.

## Results

The container ran in DFW and the bucket was in WNAM. Each R2 call took
about 190 ms (HEAD) or 300 ms (LIST). The intercept alone takes about
6 ms.

| | FUSE mount (s3fs 1.95) | Zip by HTTP ranges |
|---|---|---|
| Mount, per start | 0.66 to 1.0 s | none |
| List 10,000 loose files | 120 s cold (45.5 s with `multireq_max=100`), about 28 s warm | n/a |
| Open the zip and list 10,001 entries | 0.93 s | 1.01 s (4 requests) |
| Read a 1.5 GiB file in order | 50.5 to 57.6 MB/s | 19 to 22 MB/s on one stream; 83 MB/s with 4 ranges at once, 158 MB/s with 8 |
| 1 MiB random reads | p50 568 ms, max 5.7 s | p50 134 ms, max 217 ms |

Every checksum matched.
