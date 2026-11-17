package hxn.audio;

#if macro
import haxe.io.Path;
import haxe.macro.Compiler;
import haxe.macro.Context;
import sys.FileSystem;
import sys.io.File;
#end

/** Copies the Worklet and its DSP module for every wasm runtime. */
class Init {
	public static macro function register():haxe.macro.Expr {
		final triple = Context.definedValue("hxn_triple");
		if (Context.defined("android") || (triple != null && triple.indexOf("android") >= 0))
			Compiler.define("hxn_audio_android");
		if (Context.defined("android")) {
			final api = Context.definedValue("HXCPP_ANDROID_PLATFORM");
			if (api == null || Std.parseInt(api) < 26)
				Compiler.define("HXCPP_ANDROID_PLATFORM", "26");
		}
		if (Context.defined("cpp") && (Context.defined("iphonesim") || Context.defined("iphoneos"))) {
			final linked = Context.definedValue("AUDIO_LINK");
			if (linked != null) {
				final document = Xml.parse(File.getContent(linked));
				for (target in document.firstElement().elements())
					for (entry in target.elements()) {
						if (entry.nodeName == "lib" && StringTools.startsWith(entry.get("name"), "-l")) {
							final name = entry.get("name");
							entry.nodeName = "flag";
							entry.remove("name");
							entry.set("value", "-ref" + name);
						} else if (entry.nodeName == "flag" && entry.get("value") == "-framework")
							entry.set("value", "-ref-framework");
					}
				final out = FileSystem.absolutePath(Compiler.getOutput());
				FileSystem.createDirectory(out);
				final path = Path.join([out, "hxn_audio_link.xml"]);
				File.saveContent(path, document.toString());
				Compiler.define("AUDIO_LINK", path);
			}
		}
		if (Context.defined("emscripten") || Context.defined("wasm")) {
			var root = Context.resolvePath("hxn/audio/Init.hx");
			for (_ in 0...4)
				root = Path.directory(root);
			Context.onAfterGenerate(() -> {
				final out = Compiler.getOutput();
				final directory = FileSystem.absolutePath(Context.defined("cpp") ? out : Path.directory(out));
				FileSystem.createDirectory(directory);
				for (name in ["hxn_audio.mjs", "hxn_audio_worklet.mjs", "hxn_audio_dsp.wasm"]) {
					var from = Path.join([root, "page", name]);
					if (!FileSystem.exists(from))
						from = Path.join([root, "assets", name]);
					if (!FileSystem.exists(from))
						Context.fatalError('missing audio asset $name; build it with hxn audio dsp', Context.currentPos());
					File.copy(from, Path.join([directory, name]));
				}
			});
		}
		return macro null;
	}
}
