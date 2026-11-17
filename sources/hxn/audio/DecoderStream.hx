package hxn.audio;

#if hxn_av
import haxe.io.Bytes;
import hxn.av.AudioDecoder;
import hxn.av.StreamReadStatus;

/** Bounded decoder-to-output transfer on the application thread. */
class DecoderStream {
	public final stream:AudioStream;
	public var transferredFrames(default, null) = 0;

	final decoder:AudioDecoder;
	final sampleRate:UInt;
	final channels:UInt;
	var pending:Null<Bytes>;
	var offset = 0;
	var ended = false;

	/** The caller retains ownership of the decoder and output. */
	public function new(output:AudioOutput, decoder:AudioDecoder, sampleRate:UInt, channels:UInt, capacityFrames:UInt, options:PlayOptions) {
		this.decoder = decoder;
		this.sampleRate = sampleRate;
		this.channels = channels;
		stream = output.stream(sampleRate, channels, capacityFrames, options);
	}

	/** Transfers at most maxBlocks decoded blocks; returns true after output drains. */
	public function pump(maxBlocks:Int = 8):Bool {
		if (maxBlocks < 1 || maxBlocks > 64)
			throw "pump block count must be 1..=64";
		if (ended)
			return stream.finished();
		var read = 0;
		while (read < maxBlocks) {
			if (pending != null) {
				final count = stream.write(pending.sub(offset, pending.length - offset));
				offset += count * channels * 4;
				transferredFrames += count;
				if (offset < pending.length)
					return false;
				pending = null;
				offset = 0;
			}
			switch decoder.poll() {
				case StreamReadStatus.Pending:
					return false;
				case StreamReadStatus.Ended:
					stream.finish();
					ended = true;
					return stream.finished();
				case StreamReadStatus.Ready:
					final data = decoder.read();
					try {
						if (data.sampleRate() != sampleRate || data.numberOfChannels() != haxe.Int64.ofInt(channels))
							throw "decoder format changed; open a matching output stream";
						pending = Av.copy(data);
						data.close();
					} catch (error:Dynamic) {
						data.close();
						throw error;
					}
					read++;
				case _:
					throw "unknown decoder status";
			}
		}
		return false;
	}

	public function close():Void {
		pending = null;
		stream.stop();
		stream.close();
	}
}
#end
