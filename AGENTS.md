# Adam's daily-driver setup

As of 2026-09-16:

Pipeline: **Omarchy microphone → remote WSL speech-to-text → remote WSL text cleanup → Omarchy typed output.** Both remote stages have local CPU fallbacks.

Project docs:

- [Contributing guide](https://github.com/peteonrails/voxtype/blob/dev/CONTRIBUTING.md)
- [Code of Conduct](https://github.com/peteonrails/voxtype/blob/dev/CODE_OF_CONDUCT.md)

- This checkout powers live dictation on Omarchy. The `voxtype-bin` package is removed.
- Run `voxtype-rebuild` after changes. It builds release binaries, installs binaries/helpers under `~/.local/lib/voxtype` and QML under `~/.local/share/voxtype`, then restarts the running user service. It does not pull updates. `~/.local/bin/voxtype` is the installed entry point.
- Keep the single config at `~/.config/voxtype/config.toml`; models/data live in `~/.local/share/voxtype`.
- Audio goes through local `voxtype-fallback-proxy.service` on port 18080 to `http://lan-party.tail928ade.ts.net:8080`. Windows forwards to WSL `archlinux`, where `voxtype-whisper.service` runs CUDA Whisper `large-v3-turbo`. Remote failure falls back to local CPU Whisper `base.en`.
- Cleanup: `[output.post_process]` → `~/.local/bin/voxtype-s1-clean` → `lan-party.tail928ade.ts.net:11434`, forwarded to WSL `voxtype-cleanup.service`. Ollama keeps `s1-mini` on the GPU. The wrapper sets `Host: localhost:11434` for forwarding, tries remote for up to 4s, then local Ollama within a 14s total budget. If both fail, VoxType preserves the original text. These scripts/services live outside this checkout.
- Windows task `VoxtypeWhisperServer` starts WSL at boot and retries every minute while down. WSL cold-start recovery was tested; a full Windows reboot was not.
- Cleanup starts and warms automatically in WSL; `VoxtypeCleanupPortProxy` maintains Tailscale forwarding. Check cleanup source/timing with `journalctl -t voxtype-cleanup`; benchmark and rollback notes are in `~/.local/state/voxtype/remote-cleanup/`.
- Diagnose with `journalctl --user -u voxtype -u voxtype-fallback-proxy`. A daemon log saying “remote transcription completed” can still mean the proxy used local fallback; check proxy logs and the WSL service journal over `ssh lp`.
