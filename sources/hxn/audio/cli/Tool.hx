package hxn.audio.cli;

import haxe.Json;
import haxe.io.Path;
import sys.FileSystem;
import sys.io.File;

/** Installed CLI commands and source build hooks for the audio library. */
class Tool {
	static function main():Void {
		try {
			final request:Dynamic = Json.parse(Sys.stdin().readAll().toString());
			if (request.version != 1)
				throw "unsupported hxn plugin protocol";
			final root:String = request.root;
			switch (request.kind : String) {
				case "hook":
					final source = FileSystem.exists(Path.join([root, "Cargo.toml"]));
					final wasm = request.context.target != null && request.context.target.wasm == true;
					if (source && (request.phase == "before-package" || wasm))
						dsp(root);
					if (request.phase == "before-build" && wasm) {
						Sys.println(Json.stringify({version: 1, page_files: [Path.join([root, "assets", "hxn_audio_dsp.wasm"])]}));
					} else
						Sys.println('{"version":1}');
				case "command":
					final args:Array<String> = request.arguments;
					switch args.shift() {
						case "doctor":
							final assets = Path.join([root, "page", "hxn_audio_dsp.wasm"]);
							final source = Path.join([root, "assets", "hxn_audio_dsp.wasm"]);
							Sys.println('AudioWorklet DSP: ${FileSystem.exists(assets) || FileSystem.exists(source) ? "installed" : "needs source build"}');
							Sys.println("Browser output requires a secure origin with COOP/COEP and playback activation.");
						case "dsp": dsp(root);
						case null | "help" | "--help":
							Sys.println("hxn audio doctor");
							Sys.println("hxn audio dsp (source checkout)");
						case command: throw 'unknown audio command: $command';
					}
				case _:
					throw "unknown hxn plugin request";
			}
		} catch (error:Dynamic) {
			Sys.stderr().writeString('hxn-audio: ${Std.string(error)}\n');
			Sys.exit(Std.isOfType(error, CommandFailure) ? (cast error : CommandFailure).code : 2);
		}
	}

	static function execute(command:String, args:Array<String>):Void {
		final code = Sys.command(command, args);
		if (code != 0)
			throw new CommandFailure(code, command);
	}

	static function dsp(root:String):Void {
		if (!FileSystem.exists(Path.join([root, "crates", "hxn-audio-dsp", "Cargo.toml"])))
			throw "DSP compilation needs an hxn-audio source checkout";
		final previous = Sys.getEnv("RUSTFLAGS");
		Sys.putEnv("RUSTFLAGS",
			"-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--max-memory=536870912 -C link-arg=--initial-memory=16777216 -C link-arg=--export=__stack_pointer");
		final target = Path.join([root, "target", "audio-dsp"]);
		execute("cargo", [
			"build",
			"--locked",
			"--manifest-path",
			Path.join([root, "Cargo.toml"]),
			"--target-dir",
			target,
			"-p",
			"hxn-audio-dsp",
			"--release",
			"--target",
			"wasm32-unknown-unknown",
			"-Z",
			"build-std=std,panic_abort"
		]);
		Sys.putEnv("RUSTFLAGS", previous);
		final assets = Path.join([root, "assets"]);
		FileSystem.createDirectory(assets);
		File.copy(Path.join([target, "wasm32-unknown-unknown", "release", "hxn_audio_dsp.wasm"]), Path.join([assets, "hxn_audio_dsp.wasm"]));
	}
}

private class CommandFailure extends haxe.Exception {
	public final code:Int;

	public function new(code:Int, command:String) {
		super('command failed ($code): $command');
		this.code = code;
	}
}
