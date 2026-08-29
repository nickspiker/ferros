package com.callrec;

import android.media.AudioManager;
import android.util.Log;

/** Watches the audio mode; starts/stops a Recorder around each in-call period. */
public class Poller implements Runnable {
    @Override public void run() {
        final int MODE_IN_COMMUNICATION = 3;
        boolean recording = false;
        Recorder rec = null;
        while (CallRecApp.running) {
            try {
                int mode = CallRecApp.am != null ? CallRecApp.am.getMode() : 0;
                if (mode == MODE_IN_COMMUNICATION && !recording) {
                    Log.i(CallRecApp.T, "mode=IN_COMMUNICATION -> start recording");
                    rec = new Recorder();
                    new Thread(rec).start();
                    recording = true;
                } else if (mode != MODE_IN_COMMUNICATION && recording) {
                    Log.i(CallRecApp.T, "mode=" + mode + " -> stop recording");
                    if (rec != null) rec.stop = true;
                    recording = false;
                }
                Thread.sleep(400);
            } catch (Throwable e) {
                Log.e(CallRecApp.T, "poller err", e);
                try { Thread.sleep(1000); } catch (Throwable ignore) {}
            }
        }
    }
}
