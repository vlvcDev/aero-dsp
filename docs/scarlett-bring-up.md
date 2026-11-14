# Scarlett and class-compliant interface bring-up

## Reference path

V0 treats the Scarlett 2i2 as a standard duplex USB Audio Class device. No generation-specific control protocol is required for basic guitar capture and stereo playback.

1. Update the Scarlett firmware from a supported macOS or Windows computer if needed.
2. Connect it directly to a Raspberry Pi 5 USB port for initial testing; avoid hubs until the baseline is stable.
3. Confirm the interface appears in both `arecord -l` and `aplay -l`.
4. Run `cargo run -p pedal-engine --bin pedal-audio-probe`.
5. If automatic selection is wrong, set `PEDAL_AUDIO_DEVICE` to the ALSA PCM name shown by the probe.
6. Run `cargo run -p pedal-engine --bin pedal-audio-loopback --release` and verify input 1 reaches both outputs.
7. Measure loopback first at 48 kHz / 64 frames. The engine automatically negotiates 128 frames if 64 is rejected.

## Channel policy

- Both hardware inputs are opened so the ALSA stream matches the interface layout.
- Guitar input defaults to zero-based channel `0` (Scarlett input 1).
- The processed mono guitar signal will feed both playback channels.
- Direct Monitor should be disabled during latency measurement, otherwise the analog monitor path can be mistaken for processed output.

## V0 portability contract

An alternative interface is compatible when ALSA exposes one duplex PCM endpoint that accepts two capture channels, two playback channels, signed 32-bit interleaved samples, and 48 kHz. The engine tries 64-frame periods first and 128 frames second. Unsupported vendor mixer features do not prevent basic audio operation.

The probe attempts to hardware-link the ALSA capture and playback streams. Direct hardware devices commonly support this; PipeWire and some ALSA plugins do not. Lack of stream-link support is reported but is not fatal—the diagnostic can operate the two streams independently.

## Realtime scheduling

The audio thread requests SCHED_FIFO priority 70 (`PEDAL_RT_PRIORITY`, 1-99;
0 disables) and the DSP workers run one step below it, so normal system work
cannot delay an audio period. The systemd unit's `LimitRTPRIO=95` grants this.
When running by hand, the engine logs once if the request is refused and keeps
normal priority; grant it with an rtprio entry for one of your groups in
`/etc/security/limits.d` (for example `@audio - rtprio 95`, then log in again).
The `meters` message reports the obtained priorities
(`audio_realtime_priority`, `dsp_worker_realtime_priority`).
