package;

import haxe.Json;
import haxe.io.Path;
import sys.FileSystem;
import sys.io.File;

/** Builds, signs and removes the temporary physical-iOS test application. */
@:access(Tool)
class IosDevice {
	static final bundle = "dev.hxn.audio.check";

	public static function run(root:String, out:String, runtime:String, device:Null<String>, team:Null<String>, host:String, words:Array<String>,
			av:Bool):Void {
		if (device == null)
			throw "physical iOS checks need --device <UDID>";
		if (team == null || !~/^[A-Z0-9]{10}$/.match(team))
			throw "physical iOS checks need --team <development-team-ID> or HXN_APPLE_TEAM";
		final info = Path.join([out, "device.json"]);
		Tool.capture("xcrun", [
			"devicectl",
			"device",
			"info",
			"details",
			"--device",
			device,
			"--json-output",
			info
		]);
		final details:Dynamic = Json.parse(File.getContent(info)).result;
		if (details.hardwareProperties.reality != "physical" || details.hardwareProperties.platform != "iOS")
			throw "choose a physical iOS device";
		if (details.deviceProperties.developerModeStatus != "enabled")
			throw "enable Developer Mode on the iPhone before running the check";
		final project = Path.join([out, "device-app"]);
		final xcode = Path.join([project, "AudioCheck.xcodeproj"]);
		FileSystem.createDirectory(xcode);
		for (name in ["host.mm", "Info.plist"])
			File.copy(Path.join([root, "ios", name]), Path.join([project, name]));
		var embedded = "";
		var phase = "";
		var link = "";
		if (runtime == "hl") {
			final framework = Path.join([project, "AudioAsh.framework"]);
			FileSystem.createDirectory(framework);
			File.saveContent(Path.join([framework, "Info.plist"]),
				'<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>AudioAsh</string><key>CFBundleIdentifier</key><string>dev.hxn.audio.runtime</string><key>CFBundlePackageType</key><string>FMWK</string><key>CFBundleVersion</key><string>1</string><key>CFBundleShortVersionString</key><string>1.0</string><key>MinimumOSVersion</key><string>14.0</string></dict></plist>');
			final previous = Sys.getEnv("ASH_LINK_ARGS");
			Sys.putEnv("ASH_LINK_ARGS",
				(previous ?? "") +
				" -dynamiclib -Wl,-install_name,@rpath/AudioAsh.framework/AudioAsh -Wl,-no_compact_unwind -framework AudioToolbox -framework CoreAudio -framework Foundation -framework AVFAudio -lobjc" +
				(av ? " -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework CoreFoundation -framework AVFoundation -framework MediaToolbox" : ""));
			var args = words.concat([
				"--build",
				Path.join([framework, "AudioAsh"]),
				"--target",
				"aarch64-apple-ios",
				"--native-lib",
				Path.join([out, "libaudio.a"])
			]);
			if (av)
				args = args.concat(["--native-lib", Path.join([out, "libxavi.a"])]);
			args.push(Path.join([out, "main.hl"]));
			try
				Tool.execute(host, args)
			catch (error:Dynamic) {
				Sys.putEnv("ASH_LINK_ARGS", previous);
				throw error;
			}
			Sys.putEnv("ASH_LINK_ARGS", previous);
			phase = "A0000000000000000000000E";
			embedded = 'A0000000000000000000000E = {isa = PBXCopyFilesBuildPhase; buildActionMask = 2147483647; dstPath = ""; dstSubfolderSpec = 10; files = (A0000000000000000000000F); name = "Embed Frameworks"; runOnlyForDeploymentPostprocessing = 0;}; A0000000000000000000000F = {isa = PBXBuildFile; fileRef = A00000000000000000000010; settings = {ATTRIBUTES = (CodeSignOnCopy,RemoveHeadersOnCopy);};}; A00000000000000000000010 = {isa = PBXFileReference; lastKnownFileType = wrapper.framework; path = AudioAsh.framework; sourceTree = SOURCE_ROOT;};';
		} else {
			final archives = FileSystem.readDirectory(out)
				.filter(name -> StringTools.startsWith(name, "libPlaybackCheck") && StringTools.endsWith(name, ".a"));
			if (archives.length != 1)
				throw "hxcpp produced no unique device archive";
			File.copy(Path.join([out, archives[0]]), Path.join([project, "program.a"]));
			link = '"$(SRCROOT)/program.a"';
		}
		final template = File.getContent(Path.join([root, "ios", "project.pbxproj"]));
		final pbx = StringTools.replace(StringTools.replace(StringTools.replace(StringTools.replace(StringTools.replace(template, "__TEAM__", team),
			"__CPP__", runtime == "cpp" ? "1" : "0"), "__EMBED_PHASE__", phase),
			"__EMBED_OBJECTS__", embedded),
			"__PROGRAM_LINK__", link);
		File.saveContent(Path.join([xcode, "project.pbxproj"]), pbx);
		final derived = Path.join([out, "xcode"]);
		Tool.execute("xcodebuild", [
			"-project",
			xcode,
			"-scheme",
			"AudioCheck",
			"-configuration",
			"Debug",
			"-sdk",
			"iphoneos",
			"-destination",
			"generic/platform=iOS",
			"-derivedDataPath",
			derived,
			"-allowProvisioningUpdates",
			"build"
		]);
		final app = Path.join([derived, "Build", "Products", "Debug-iphoneos", "AudioCheck.app"]);
		Tool.execute("xcrun", [
			"devicectl",
			"device",
			"install",
			"app",
			"--device",
			device,
			app,
			"--timeout",
			"60"
		]);
		try {
			final log = Tool.capture("xcrun", [
				"devicectl",
				"device",
				"process",
				"launch",
				"--device",
				device,
				"--terminate-existing",
				"--console",
				"--timeout",
				"60",
				"--json-output",
				Path.join([out, "launch.json"]),
				bundle
			]);
			Sys.print(log);
			if (log.indexOf("audio playback: passed") < 0 || log.indexOf("audio playback failed:") >= 0)
				throw "physical iOS app did not complete its playback assertions";
		} catch (error:Dynamic) {
			Sys.command("xcrun", [
				"devicectl",
				"device",
				"uninstall",
				"app",
				"--device",
				device,
				bundle,
				"--timeout",
				"30"
			]);
			throw error;
		}
		Tool.execute("xcrun", [
			"devicectl",
			"device",
			"uninstall",
			"app",
			"--device",
			device,
			bundle,
			"--timeout",
			"30"
		]);
	}
}
