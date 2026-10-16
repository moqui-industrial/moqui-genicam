# Validation — 2026-10-16

Tested against MR-Moqui master ab88f80, Rust 1.99.1 and a disposable PostgreSQL
18.1 database, without mantle-udm. Python mock acquisition uses NumPy/OpenCV;
no Harvesters installation or physical camera is involved.

- Six component features passed: image bytes/timestamp/snapshot references,
  image DeviceContent persistence, PhysicalDevice serial fallback, 3D Tensor/NPY
  persistence, connection ownership and foreign-data isolation during retention.
- Three Rust unit tests passed: numeric timestamps, persistent camera cache,
  and termination of a stalled worker without replaying its request.
- Three Python tests passed: explicit simulation, preserved frame age after
  resize/re-encoding, and multi-camera worker deadlines.
- Component Clippy with all targets and warnings denied passed.
- XML schema, service artifact and data-model validation passed.
- Combined device/GenICam execution passed all 18 features without Mantle,
  including native ONNX inference and the optional loopback HTTP contracts.

Corrections cover the base64 IPC format, per-camera process lifetime, worker
shutdown/deadlines, status field, physical serial numbers, runtime path resolution,
image reference persistence and scoped tensor retention. MoquiConf.xml default
properties precede tool declarations as required by the framework schema.

Filesystem writes/deletions are not transactional with the database. Retention
keeps Tensor/Axis metadata and skips other components' files/custom output roots.
The maintainer reports prior Python-path validation with two real FLIR cameras.
This report records the Rust/mock checks separately: it does not repeat or
invalidate that hardware work. Rust SDK lifecycle and sustained frame throughput
still need hardware measurements. The obstacle detector/calibration stage is
simulated by the separate moqui-device ONNX example.

MJPEG follow-up: a real loopback HTTP test passed with the new native response
stream, the persistent Python worker and explicit mock camera. It verified two
progressively delivered JPEG parts separated by the requested delay, finite
closing boundary, unlimited-stream disconnect and a subsequent camera request.
The distributed screen was also exercised with an authenticated browser session
and explicit application artifact permissions: anonymous access was rejected,
and the finite/progressive/unlimited/disconnect checks passed with that session.
The worker test additionally verifies that closing a camera permits reopening
but closing the factory rejects subsequent capture requests. Framework tests
passed for nested output/stream handling (2) and on-demand host body disposal (1). Screen
and ToolFactory configuration XSD validation passed. No physical-camera MJPEG
throughput claim is made.
