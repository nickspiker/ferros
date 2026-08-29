package com.callrec;

import android.app.Application;
import android.content.Context;
import android.media.AudioManager;
import android.util.Log;

/**
 * Auto call recorder — Phase 1 validation build.
 *
 * Persistent priv-app. Polls the audio mode; when a VoIP/telephony call goes active
 * (MODE_IN_COMMUNICATION) it records via a single AudioRecord(VOICE_CALL) to a WAV and
 * logs RMS so we can confirm we actually captured audio (and which direction). Once the
 * tap is proven, Phase 2 adds dual-stream stereo split + Opus + real storage.
 *
 * Flat top-level classes only — the bundled d8 NPEs on inner classes.
 */
public class CallRecApp extends Application {
    static final String T = "CALLREC";
    static AudioManager am;
    static Context ctx;
    static String dir;
    static volatile boolean running = true;

    @Override public void onCreate() {
        super.onCreate();
        ctx = getApplicationContext();
        am = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
        try {
            java.io.File rec = new java.io.File(android.os.Environment.getExternalStorageDirectory(), "Recordings");
            rec.mkdirs();
            dir = rec.getAbsolutePath();
        } catch (Throwable t) { dir = "/storage/emulated/0/Recordings"; }
        Log.i(T, "onCreate — files dir=" + dir + " — starting mode poller");
        Thread t = new Thread(new Poller());
        t.setDaemon(true);
        t.start();
    }
}
