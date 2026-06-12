# Vendored `qdl`

This is a vendored copy of the `qdl` crate from
[`qualcomm/qdlrs`](https://github.com/qualcomm/qdlrs) (Sahara / Firehose
EDL transport), licensed BSD-3-Clause (see `LICENSE`).

## Why vendored

The EDL flash path needs fixes that are not yet in an upstream release,
and `qualcomm/qdlrs` is not pushable by us. Vendoring keeps the build
reproducible (no external fork repo, no submodule fetch in CI) while
carrying the minimal patches we need.

## Source

- Upstream: `qualcomm/qdlrs` at `cdec5ea`
  (`Merge pull request #44 … sahara-archive`).

## Local patches

- **Drop the redundant explicit ZLP in `firehose_program_storage`**
  (`src/lib.rs`). The USB `Write` impl already terminates every transfer
  via `EndpointWrite::submit_end()` — a zero-length packet when the
  payload is a multiple of the bulk max-packet size, a short packet
  otherwise. The extra explicit `channel.write(&[])` put a second, stray
  zero-length OUT transfer on the wire; after a packet-aligned partition
  Firehose has already byte-counted all its sectors and stops reading the
  OUT endpoint, so that stray ZLP stalls the next `<program>` write
  indefinitely (the endpoint write timeout does not cancel the queued
  transfer). Symptom: a multi-partition flash hung on the partition after
  the first packet-aligned one (e.g. `xbl_config_a`, 245760 B = exact
  512-multiple).
- **Make the serial backend tolerant enough for Qualcomm kernel-driver mode**
  (`src/serial.rs`). LTBox opens the port with an identity configuration,
  applies raw mode + 115200 baud best-effort, and sets explicit read/write
  timeouts. This keeps the serial path usable when the user selects Qualcomm's
  kernel driver family while avoiding hard failure on hosts whose serial
  driver rejects one of the advisory termios settings.

## Updating

To re-sync with upstream: re-copy `src/` + `Cargo.toml` from the desired
`qualcomm/qdlrs` revision, then re-apply the patches above. Update the
revision recorded here.
