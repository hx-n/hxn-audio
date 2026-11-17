package;

import haxe.io.Bytes;
import hxn.audio.*;

/** Exercises the actual output stream; the timer never mixes samples. */
#if (cpp && android)
@:cppFileCode("#include <android/log.h>")
@:buildXml('<target id="haxe"><flag value="-Wl,-u,hxn_audio_check_context"/></target>')
#end
class PlaybackCheck {
	static inline final toneFrames = #if hxn_audio_audible 240000 #else 4800 #end;
	static inline final musicFrames = #if hxn_audio_audible 192000 #else 12000 #end;
	static inline final amplitude = #if hxn_audio_audible 0.12 #else 0.015 #end;
	static var output:AudioOutput;
	static var stream:AudioStream;
	static var first:AudioVoice;
	static var second:AudioVoice;
	static var music:Bytes;
	static var written = 0;
	static var ending = false;
	static var ticks = 0;
	static var timer:haxe.Timer;
	static var paused = false;
	static var pausedAt = 0;
	static var resumed = false;
	static var finished = false;
	static var peak = 0.0;
	static var activationAsked = false;

	static function main():Void {
		try
			start()
		catch (error:Dynamic)
			fail(error);
	}

	static function start():Void {
		output = new AudioOutput({});
		#if hxn_audio_audible
		report("audio listening check: eight-second tone starting");
		#end
		final tone = Bytes.alloc(toneFrames * 4);
		for (i in 0...toneFrames)
			tone.setFloat(i * 4, Math.sin(i * 440 * Math.PI * 2 / 48000) * amplitude);
		final buffer = new AudioBuffer(tone, 48000, 1);
		first = output.play(buffer, {pan: -1});
		second = output.play(buffer, {pan: 1, bus: 1});
		buffer.close();
		final bus = output.bus(1);
		bus.setGain(0.5);
		bus.setDelay(240, 0.2, 0.1);
		bus.close();
		#if hxn_audio_av
		stream = DecoderCheck.open(output);
		#else
		music = Bytes.alloc(musicFrames * 4);
		for (i in 0...musicFrames)
			music.setFloat(i * 4, Math.sin(i * 220 * Math.PI * 2 / 24000) * amplitude);
		stream = output.stream(24000, 1, 4096, {});
		#end
		#if emscripten
		hxn.native.Emscripten.later(frame);
		#else
		timer = new haxe.Timer(10);
		timer.run = frame;
		#end
	}

	static function frame():Void {
		try
			tick()
		catch (error:Dynamic)
			fail(error);
		#if emscripten
		if (!finished)
			hxn.native.Emscripten.later(frame);
		#end
	}

	static function tick():Void {
		if (++ticks > 3000)
			throw "audio output timed out; activate playback with a click or key";
		final metrics = output.metrics();
		if (metrics.state == AudioState.Error)
			throw metrics.error;
		peak = Math.max(peak, metrics.peak);
		#if hxn_audio_trace
		if (ticks % 100 == 1)
			report('audio progress: tick=$ticks state=${metrics.state} frames=${metrics.renderedFrames} callbacks=${metrics.callbacks} peak=$peak ending=$ending');
		#end
		#if (wasm || emscripten)
		if (!paused && !activationAsked && metrics.state == AudioState.Suspended) {
			activationAsked = true;
			report("audio activation required");
		}
		#end
		#if hxn_audio_av
		ending = DecoderCheck.pump();
		#else
		if (written < musicFrames) {
			final count = stream.write(music.sub(written * 4, (musicFrames - written) * 4));
			written += count;
		} else if (!ending) {
			stream.finish();
			ending = true;
		}
		#end
		if (metrics.renderedFrames > haxe.Int64.ofInt(4800) && !paused) {
			output.pause();
			paused = true;
			pausedAt = ticks;
			return;
		}
		if (paused && !resumed) {
			if (ticks - pausedAt < 5)
				return;
			if (metrics.state != AudioState.Suspended)
				throw "output did not suspend";
			#if (!wasm && !emscripten)
			output.selectDevice(metrics.device);
			#end
			output.resume();
			resumed = true;
		}
		if (ending && stream.finished() && first.finished() && second.finished()) {
			final metrics = output.metrics();
			if (peak <= 0 || metrics.renderedFrames < haxe.Int64.ofInt(24000))
				throw "output did not render overlapping sounds and the music stream";
			if (!(metrics.latencySeconds > 0))
				throw "device reported no output latency estimate";
			report('audio latency estimate: ${metrics.latencySeconds} s');
			#if hxn_audio_av
			DecoderCheck.close();
			#else
			stream.close();
			#end
			first.close();
			second.close();
			output.close();
			finished = true;
			#if !emscripten
			timer.stop();
			#end
			report("audio playback: passed");
			#if !(hl && hxn_audio_android)
			hxn.Loop.exit(0);
			#end
		}
	}

	static function report(message:String):Void {
		#if (cpp && android)
		untyped __cpp__('__android_log_write(ANDROID_LOG_INFO, "hxn-audio-check", {0}.utf8_str())', message);
		#else
		Sys.println(message);
		#end
	}

	static function fail(error:Dynamic):Void {
		report('audio playback failed: ${Std.string(error)}');
		finished = true;
		#if !emscripten
		if (timer != null)
			timer.stop();
		#end
		if (output != null)
			output.close();
		#if !(hl && hxn_audio_android)
		hxn.Loop.exit(1);
		#end
	}
}
