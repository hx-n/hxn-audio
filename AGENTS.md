# Working on hxn-audio

Read docs/design.md, docs/conventions.md and skills/hxn-audio/SKILL.md.
Use the hxn repository's git-bug tracker for hxnme integration work. Read
its audio issue before changing this library; keep measurements, pending
work and decisions there rather than in status or planning documents.

Before calling a change done, run cargo fmt --all, cargo clippy --workspace
--all-targets and cargo test --workspace. Run hxn check on HashLink, Ash
hybrid/JIT/interpreter, hxcpp and CPPIA. Run actual playback through
hxn audio check on desktop, iOS and Android, and in a browser through Ash,
unthreaded hxcpp and threaded hxcpp. Repeat concurrent/browser tests under
timeouts. Report unavailable devices and runtimes, and distinguish backend
latency estimates from acoustic loopback measurements. Windows/Linux are
not verified until their CI runs.

The render path never allocates, frees, locks, logs or calls Haxe. Keep
retired resources alive until the control side reclaims them. Browser
mixing runs inside the AudioWorklet, including when Haxe has no threads.
Reuse hxn-av for PCM conversion and decoding. Public workflows use hxn CLI
commands and shipped Haxe tools, without repository shell helpers.

Keep the skill and convention/design docs current with API or workflow
changes. Run Haxe formatter on changed Haxe files. Do not commit, push,
publish or push git-bug metadata unless asked. Commit messages cite the
full git-bug ID in a git-bug trailer, without Closes/Fixes or attribution.
