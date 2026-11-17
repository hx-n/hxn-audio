package hxn.audio.cli;

#if hxn_audio_av
import haxe.Int64;
import haxe.crypto.Base64;
import hxn.av.*;
import hxn.audio.*;

/** Feeds AAC packets incrementally and retries bounded decoder/stream writes. */
class DecoderCheck {
	static var decoder:AudioDecoder;
	static var bridge:DecoderStream;
	static var packets:Array<Dynamic>;
	static var index = 0;
	static var pending:EncodedAudioChunk;
	static var finished = false;
	static var checked = false;
	static var ticks = 0;

	public static function open(output:AudioOutput):AudioStream {
		final fixture:Dynamic = haxe.Json.parse(haxe.Resource.getString("hxn_audio_aac"));
		packets = fixture.packets;
		final config = CodecConfiguration.audio("mp4a.40.2", Int64.ofInt(fixture.sampleRate), Int64.ofInt(fixture.channels),
			Base64.decode(fixture.description));
		decoder = AudioDecoder.create(8, Int64.ofInt(65536), config);
		config.close();
		// A block exceeds this ring, exercising suffix retries on every block.
		bridge = new DecoderStream(output, decoder, fixture.sampleRate, fixture.channels, 257, {});
		return bridge.stream;
	}

	public static function pump():Bool {
		#if hxn_audio_trace
		if (++ticks % 100 == 1)
			Sys.println('decode progress: packets=$index frames=${bridge.transferredFrames} finished=$finished');
		#end
		for (_ in 0...4) {
			if (index >= packets.length)
				break;
			if (pending == null) {
				final packet = packets[index];
				final init = new EncodedAudioChunkInit(EncodedChunkType.Key, Int64.ofInt(packet.timestamp), Base64.decode(packet.data));
				pending = EncodedAudioChunk.create(init);
				init.close();
			}
			if (!decoder.tryDecode(pending))
				break;
			pending.close();
			pending = null;
			index++;
		}
		if (index == packets.length && !finished) {
			decoder.finish();
			finished = true;
		}
		final drained = bridge.pump();
		if (drained && !checked) {
			if (bridge.transferredFrames < 24000)
				throw "AAC decoder produced too few samples";
			if (Math.abs(bridge.stream.positionSeconds() * 48000 - bridge.transferredFrames) > 0.5)
				throw 'decoder stream lost samples during partial writes: ${bridge.stream.positionSeconds() * 48000} consumed, ${bridge.transferredFrames} written';
			Sys.println('decoded audio: ${bridge.transferredFrames} frames, partial writes preserved');
			checked = true;
		}
		return drained;
	}

	public static function close():Void {
		if (pending != null)
			pending.close();
		bridge.close();
		decoder.close();
	}
}
#end
