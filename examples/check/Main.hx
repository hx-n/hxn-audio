import haxe.io.Bytes;
import hxn.audio.*;

class Main {
	static function ensure(value:Bool, message:String):Void {
		if (!value)
			throw message;
	}

	static function main():Void {
		final output = AudioOutput.offline({});
		final pcm = Bytes.alloc(480 * 4);
		for (i in 0...480)
			pcm.setFloat(i * 4, 0.1);
		final buffer = new AudioBuffer(pcm, 48000, 1);
		final options:PlayOptions = {bus: 1};
		final left = output.play(buffer, options);
		final right = output.play(buffer, {});
		buffer.close();
		left.setPan(-1);
		right.setPan(1);
		final master = output.bus(0);
		final bus = output.bus(1);
		bus.routeTo(master);
		bus.setGain(0.5);
		final rendered = Bytes.alloc(1024 * 8);
		output.render(rendered);
		ensure(left.finished() && right.finished(), "overlap did not finish");
		ensure(rendered.getFloat(256 * 8) > 0.04, "left voice missing");
		ensure(rendered.getFloat(256 * 8 + 4) > 0.09, "right voice missing");
		final stream = output.stream(24000, 1, 128, {});
		var accepted = 0;
		while (accepted < 480) {
			final count = stream.write(pcm.sub(accepted * 4, (480 - accepted) * 4));
			accepted += count;
			output.render(Bytes.alloc(128 * 8));
		}
		stream.finish();
		for (_ in 0...10)
			output.render(Bytes.alloc(128 * 8));
		ensure(stream.finished(), "stream did not drain");
		ensure(output.metrics().activeVoices == 0, "voice leaked");
		stream.close();
		left.close();
		right.close();
		bus.close();
		master.close();
		output.close();
		Sys.println("audio ok");
	}
}
