package hxn.audio.cli;

import android.app.Activity;
import android.os.Bundle;

/** Activity host for the installed audio playback check. */
public final class CheckActivity extends Activity implements Runnable {
    static { System.loadLibrary("hxn_audio_check"); }
    private static final java.util.concurrent.atomic.AtomicBoolean started = new java.util.concurrent.atomic.AtomicBoolean();
    private static native void runCheck(android.content.Context context);
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        if (started.compareAndSet(false, true)) new Thread(this, "hxn audio check").start();
    }
    @Override public void run() { runCheck(getApplicationContext()); }
}
