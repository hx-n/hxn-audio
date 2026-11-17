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
						case "check": check(root, args);
						case null | "help" | "--help":
							Sys.println("hxn audio doctor");
							Sys.println("hxn audio check <hl|cpp> [--audible] [--av] [--wasm --browser] [--threads] [--vm <command>] [--target <triple> --device <id>]");
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

	static function check(root:String, args:Array<String>):Void {
		final runtime = args.shift() ?? "hl";
		if (runtime != "hl" && runtime != "cpp")
			throw "expected hl or cpp";
		var wasm = false;
		var threads = false;
		var browser = false;
		var audible = false;
		var av = false;
		var target:Null<String> = null;
		var device:Null<String> = null;
		var vm = Sys.getEnv("HXN_VM") ?? "ash";
		while (args.length > 0)
			switch args.shift() {
				case "--av":
					av = true;
				case "--audible":
					audible = true;
				case "--wasm":
					wasm = true;
				case "--threads":
					threads = true;
				case "--browser":
					browser = true;
				case "--target":
					target = args.shift();
					if (target == null)
						throw "--target needs a triple";
				case "--device":
					device = args.shift();
					if (device == null)
						throw "--device needs a simulator ID or adb serial";
				case "--vm":
					vm = args.shift();
					if (vm == null)
						throw "--vm needs a command";
				case flag:
					throw 'unknown option: $flag';
			}
		if (browser != wasm || (threads && !wasm))
			throw "browser checks require --wasm --browser; threads require wasm";
		if (runtime == "hl" && wasm && !threads)
			throw "Ash browser audio needs --threads";
		if (target != null && wasm)
			throw "mobile targets cannot be combined with wasm";
		if (target != null && target != "aarch64-apple-ios-sim" && target != "x86_64-apple-ios" && target != "aarch64-linux-android")
			throw "expected an iOS simulator or Android arm64 target";
		if (target != null && runtime == "cpp" && target == "aarch64-apple-ios-sim")
			throw "hxcpp simulator checks use --target x86_64-apple-ios";
		final out = Path.join([
			Sys.getCwd(),
			"target",
			"hxn-audio",
			"check",
			runtime + (wasm ? "-wasm" : "") + (threads ? "-threads" : "") + (target == null ? "" : "-" + target)
		]);
		FileSystem.createDirectory(out);
		var flags = [
			"-lib",
			"hxn",
			"-lib",
			"hxn-audio",
			"-cp",
			Path.join([root, "sources"]),
			"-main",
			"hxn.audio.cli.PlaybackCheck"
		];
		if (Sys.getEnv("HXN_AUDIO_TRACE") != null)
			flags = flags.concat(["-D", "hxn_audio_trace"]);
		if (av)
			flags = flags.concat([
				"-lib",
				"hxn-av",
				"-D",
				"hxn_audio_av",
				"-resource",
				Path.join([root, "tooling", "tone-aac.json"]) + "@hxn_audio_aac"
			]);
		if (audible)
			flags = flags.concat(["-D", "hxn_audio_audible"]);
		if (target != null)
			flags = flags.concat(["-D", "hxn_triple=" + target]);
		if (target != null && runtime == "cpp") {
			if (target == "x86_64-apple-ios")
				flags = flags.concat(["-D", "iphonesim", "-D", "HXCPP_M64"]);
			else
				flags = flags.concat([
					"-D",
					"android",
					"-D",
					"HXCPP_ARM64",
					"-D",
					"HXCPP_DEBUG_LINK",
					"-D",
					"ANDROID_HOST=" + Path.withoutDirectory(Path.directory(androidTools()))
				]);
		}
		if (threads)
			flags = flags.concat(["-D", "hxn_threads"]);
		if (runtime == "hl")
			flags = flags.concat(["-hl", Path.join([out, "main.hl"])]);
		else
			flags = flags.concat(["-cpp", out]);
		if (wasm)
			flags = flags.concat(runtime == "hl" ? ["-D", "wasm"] : [
				"-D",
				"emscripten",
				"-D",
				"disable_unicode_strings",
				"-D",
				"HXCPP_LINK_EMSCRIPTEN_EXT=.html",
				"-D",
				"HXCPP_LINK_EMRUN"
			]);
		final cc = Sys.getEnv("CC");
		final cxx = Sys.getEnv("CXX");
		if (runtime == "cpp" && wasm) {
			Sys.putEnv("CC", null);
			Sys.putEnv("CXX", null);
		}
		execute("haxe", flags);
		Sys.putEnv("CC", cc);
		Sys.putEnv("CXX", cxx);
		final words = ~/\s+/g.split(vm).filter(word -> word.length > 0);
		final host = words.shift();
		if (target != null) {
			mobile(root, out, runtime, target, device, host, words, av);
			return;
		}
		if (!wasm) {
			if (runtime == "hl")
				execute(host, words.concat([Path.join([out, "main.hl"])]));
			else
				execute(Path.join([out, "PlaybackCheck" + (Sys.systemName() == "Windows" ? ".exe" : "")]), []);
			return;
		}
		if (runtime == "hl")
			execute(host, words.concat((av ? ["--native-lib", Path.join([out, "libxavi.a"])] : []).concat([
				"--build",
				Path.join([out, "main.wasm"]),
				"--target",
				"wasm32-wasip1-threads",
				"--native-lib",
				Path.join([out, "libaudio.a"]),
				Path.join([out, "main.hl"])
			])));
		final chrome = Sys.getEnv("HXN_BROWSER") ?? (Sys.systemName() == "Mac" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
		execute("node", [
			Path.join([root, "sources", "hxn", "audio", "cli", "page.mjs"]),
			runtime == "hl" ? host : "emcc",
			out,
			chrome
		]);
	}

	static function capture(command:String, args:Array<String>):String {
		final process = new sys.io.Process(command, args);
		final text = process.stdout.readAll().toString();
		final error = process.stderr.readAll().toString();
		final status = process.exitCode();
		process.close();
		if (status != 0)
			throw new CommandFailure(status, command + ": " + error);
		return text;
	}

	static function androidTools():String {
		final ndk = Sys.getEnv("ANDROID_NDK_ROOT") ?? Sys.getEnv("ANDROID_NDK_HOME");
		if (ndk == null)
			throw "Android checks need ANDROID_NDK_ROOT";
		final prebuilt = Path.join([ndk, "toolchains", "llvm", "prebuilt"]);
		final host = FileSystem.readDirectory(prebuilt).filter(name -> FileSystem.isDirectory(Path.join([prebuilt, name])))[0];
		if (host == null)
			throw "Android NDK toolchain is unavailable";
		return Path.join([prebuilt, host, "bin"]);
	}

	static function mobile(root:String, out:String, runtime:String, target:String, device:Null<String>, host:String, words:Array<String>, av:Bool):Void {
		final ios = target.indexOf("apple") >= 0;
		final program = Path.join([out, ios ? "program" : "libhxn_audio_check.so"]);
		if (!ios && runtime == "hl") {
			execute(Path.join([androidTools(), "aarch64-linux-android26-clang"]), [
				"-fPIC",
				"-c",
				Path.join([root, "tooling", "android", "host.c"]),
				"-o",
				Path.join([out, "host.o"])
			]);
		}
		if (runtime == "hl") {
			final previous = Sys.getEnv("ASH_LINK_ARGS");
			final api = Sys.getEnv("ASH_ANDROID_API");
			if (!ios && (api == null || Std.parseInt(api) < 26))
				Sys.putEnv("ASH_ANDROID_API", "26");
			final libraries = (ios ? " -framework AudioToolbox -framework CoreAudio -framework Foundation -framework AVFAudio -lobjc" : " -laaudio -landroid -llog")
				+
				(av ? (ios ? " -Wl,-no_compact_unwind -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework CoreFoundation -framework AVFoundation -framework MediaToolbox" : " -lmediandk") : "");
			Sys.putEnv("ASH_LINK_ARGS", (previous ?? "") + libraries + (ios ? "" : " -shared -Wl,-soname,libhxn_audio_check.so"));
			var build = words.concat([
				"--build",
				program,
				"--target",
				target,
				"--native-lib",
				Path.join([out, "libaudio.a"]),
				Path.join([out, "main.hl"])
			]);
			if (av)
				build = build.concat(["--native-lib", Path.join([out, "libxavi.a"])]);
			if (!ios)
				build = build.concat(["--native-lib", Path.join([out, "host.o"])]);
			execute(host, build);
			Sys.putEnv("ASH_LINK_ARGS", previous);
			Sys.putEnv("ASH_ANDROID_API", api);
		} else if (ios) {
			final archive = FileSystem.readDirectory(out)
				.filter(name -> StringTools.startsWith(name, "libPlaybackCheck") && StringTools.endsWith(name, ".a"))[0];
			if (archive == null)
				throw "hxcpp produced no simulator archive";
			execute("xcrun", [
				"--sdk",
				"iphonesimulator",
				"clang++",
				"-arch",
				"x86_64",
				"-mios-simulator-version-min=12.0",
				Path.join([root, "tooling", "mobile.cpp"]),
				Path.join([out, archive]),
				"-framework",
				"AudioToolbox",
				"-framework",
				"CoreAudio",
				"-framework",
				"Foundation",
				"-framework",
				"AVFAudio",
				"-lobjc",
				"-liconv",
				"-o",
				program
			]);
		} else {
			execute(Path.join([androidTools(), "aarch64-linux-android26-clang"]), [
				"-fPIC",
				"-shared",
				"-D",
				"HXN_CHECK_CPP",
				Path.join([root, "tooling", "android", "host.c"]),
				"-L" + out,
				"-lPlaybackCheck-64",
				"-llog",
				"-o",
				program
			]);
		}
		if (ios) {
			if (device == null)
				throw "choose an iOS simulator with --device <UDID>";
			final list:Dynamic = Json.parse(capture("xcrun", ["simctl", "list", "devices", "--json"]));
			var state:Null<String> = null;
			for (runtime in Reflect.fields(list.devices))
				for (entry in (Reflect.field(list.devices, runtime) : Array<Dynamic>))
					if (entry.udid == device)
						state = entry.state;
			if (state == null)
				throw "iOS simulator was not found";
			final booted = state != "Booted";
			if (booted)
				execute("xcrun", ["simctl", "boot", device]);
			try {
				execute("xcrun", ["simctl", "bootstatus", device, "-b"]);
				execute("xcrun", ["simctl", "spawn", device, program]);
			} catch (error:Dynamic) {
				if (booted)
					Sys.command("xcrun", ["simctl", "shutdown", device]);
				throw error;
			}
			if (booted)
				execute("xcrun", ["simctl", "shutdown", device]);
		} else {
			androidApp(root, out, runtime, device);
		}
	}

	static function androidApp(root:String, out:String, runtime:String, device:Null<String>):Void {
		final sdk = Sys.getEnv("ANDROID_HOME") ?? Sys.getEnv("ANDROID_SDK_ROOT");
		if (sdk == null)
			throw "Android checks need ANDROID_HOME or ANDROID_SDK_ROOT";
		final platforms = FileSystem.readDirectory(Path.join([sdk, "platforms"])).filter(name -> StringTools.startsWith(name, "android-"));
		platforms.sort((a, b) -> Std.parseInt(b.substr(8)) - Std.parseInt(a.substr(8)));
		final jar = Path.join([sdk, "platforms", platforms[0], "android.jar"]);
		final tools = FileSystem.readDirectory(Path.join([sdk, "build-tools"]));
		tools.sort((a, b) -> -Reflect.compare(a, b));
		final build = Path.join([sdk, "build-tools", tools[0]]);
		final suffix = Sys.systemName() == "Windows" ? ".exe" : "";
		final batch = Sys.systemName() == "Windows" ? ".bat" : "";
		final java = Sys.getEnv("JAVA_HOME");
		final javac = java == null ? "javac" : Path.join([java, "bin", "javac" + suffix]);
		final keytool = java == null ? "keytool" : Path.join([java, "bin", "keytool" + suffix]);
		final classes = Path.join([out, "classes"]);
		final dex = Path.join([out, "dex"]);
		FileSystem.createDirectory(classes);
		FileSystem.createDirectory(dex);
		execute(javac, [
			"-source",
			"8",
			"-target",
			"8",
			"-bootclasspath",
			jar,
			"-d",
			classes,
			Path.join([root, "tooling", "android", "hxn", "audio", "cli", "CheckActivity.java"])
		]);
		execute(Path.join([build, "d8" + batch]), [
			"--min-api",
			"26",
			"--lib",
			jar,
			"--output",
			dex,
			Path.join([classes, "hxn", "audio", "cli", "CheckActivity.class"])
		]);
		final apk = Path.join([out, "check.apk"]);
		execute(Path.join([build, "aapt2" + suffix]), [
			"link",
			"--manifest",
			Path.join([root, "tooling", "android", "AndroidManifest.xml"]),
			"-I",
			jar,
			"-o",
			apk
		]);
		final stage = Path.join([out, "apk-libraries"]);
		final libraries = Path.join([stage, "lib", "arm64-v8a"]);
		FileSystem.createDirectory(libraries);
		File.copy(Path.join([out, "libhxn_audio_check.so"]), Path.join([libraries, "libhxn_audio_check.so"]));
		if (runtime == "cpp")
			File.copy(Path.join([out, "libPlaybackCheck-64.so"]), Path.join([libraries, "libPlaybackCheck-64.so"]));
		final jarTool = java == null ? "jar" : Path.join([java, "bin", "jar" + suffix]);
		execute(jarTool, ["uf", apk, "-C", dex, "classes.dex", "-C", stage, "lib"]);
		final key = Path.join([out, "check.keystore"]);
		if (!FileSystem.exists(key))
			execute(keytool, [
				"-genkeypair",
				"-keystore",
				key,
				"-storepass",
				"android",
				"-keypass",
				"android",
				"-alias",
				"check",
				"-dname",
				"CN=hxn audio check",
				"-keyalg",
				"RSA",
				"-validity",
				"3650"
			]);
		execute(Path.join([build, "apksigner" + batch]), [
			"sign",
			"--ks",
			key,
			"--ks-pass",
			"pass:android",
			"--key-pass",
			"pass:android",
			apk
		]);
		final adb = device == null ? [] : ["-s", device];
		final since = StringTools.trim(capture("adb", adb.concat(["shell", "date", "+%s.%3N"])));
		execute("adb", adb.concat(["install", "-r", apk]));
		try {
			execute("adb", adb.concat(["shell", "am", "start", "-n", "hxn.audio.cli.check/hxn.audio.cli.CheckActivity"]));
			final deadline = haxe.Timer.stamp() + 45;
			var passed = false;
			var log = "";
			while (haxe.Timer.stamp() < deadline) {
				log = capture("adb", adb.concat(["logcat", "-d", "-v", "raw", "-T", since, "-s", "hxn-audio-check:V"]));
				if (log.indexOf("audio playback failed:") >= 0 || log.indexOf("panicked at") >= 0)
					throw log;
				if (log.indexOf("audio playback: passed") >= 0) {
					passed = true;
					break;
				}
				Sys.sleep(0.5);
			}
			Sys.print(log);
			if (!passed)
				throw "Android Activity did not complete its playback assertions";
		} catch (error:Dynamic) {
			Sys.command("adb", adb.concat(["shell", "am", "force-stop", "hxn.audio.cli.check"]));
			Sys.command("adb", adb.concat(["uninstall", "hxn.audio.cli.check"]));
			throw error;
		}
		execute("adb", adb.concat(["uninstall", "hxn.audio.cli.check"]));
	}
}

private class CommandFailure extends haxe.Exception {
	public final code:Int;

	public function new(code:Int, command:String) {
		super('command failed ($code): $command');
		this.code = code;
	}
}
