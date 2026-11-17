package hxn.audio;

#if hxn_av
/** Copies decoder PCM on the application thread, outside the render callback. */
class Av {
	/** Converts through hxn-av's existing sample-format and planar conversion. */
	public static function copy(data:hxn.av.AudioData):haxe.io.Bytes {
		final options = new hxn.av.AudioDataCopyToOptions(haxe.Int64.ofInt(0));
		options.format(hxn.av.AudioSampleFormat.F32);
		try {
			final size = data.allocationSize(options);
			if (size.high != 0 || size.low < 0)
				throw "decoded audio block is too large";
			final pcm = haxe.io.Bytes.alloc(size.low);
			data.copyTo(pcm, options);
			options.close();
			return pcm;
		} catch (error:Dynamic) {
			options.close();
			throw error;
		}
	}

	public static function buffer(data:hxn.av.AudioData):AudioBuffer {
		return new AudioBuffer(copy(data), rate(data), channels(data));
	}

	/** Creates a stream matching the first decoded block; callers retain EOF. */
	public static function stream(output:AudioOutput, data:hxn.av.AudioData, capacityFrames:UInt, options:PlayOptions):AudioStream {
		return output.stream(rate(data), channels(data), capacityFrames, options);
	}

	static function rate(data:hxn.av.AudioData):UInt {
		final rate = data.sampleRate();
		if (!Math.isFinite(rate) || rate != Math.floor(rate))
			throw "audio sample rate must be an integer";
		return Std.int(rate);
	}

	static function channels(data:hxn.av.AudioData):UInt {
		final count = data.numberOfChannels();
		if (count.high != 0 || (count.low != 1 && count.low != 2))
			throw "audio output requires mono or stereo PCM";
		return count.low;
	}
}
#end
