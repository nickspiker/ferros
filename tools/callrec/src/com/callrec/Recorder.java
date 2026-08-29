package com.callrec;

import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.media.MediaMuxer;
import android.util.Log;
import java.nio.ByteBuffer;
import java.util.Arrays;

/**
 * Shippable capture: stereo dual-source -> uplink gain -> Opus (MediaCodec) -> .ogg (MediaMuxer).
 *   LEFT  = VOICE_UPLINK   (you), boosted by GAIN because the uplink tap sits low
 *   RIGHT = VOICE_DOWNLINK  (far end)
 * 48 kHz stereo, ~64 kbps VBR Opus. Each party on their own channel.
 * Flat top-level class (the bundled d8 NPEs on inner classes); MediaCodec.BufferInfo is a
 * framework nested class and is fine to reference.
 */
public class Recorder implements Runnable {
    volatile boolean stop = false;

    private MediaCodec enc;
    private MediaMuxer mux;
    private int track = -1;
    private boolean muxStarted = false;
    private final MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();

    @Override public void run() {
        final int UP = 2;     // VOICE_UPLINK
        final int DOWN = 3;   // VOICE_DOWNLINK
        final int RATE = 48000;
        final int MONO = AudioFormat.CHANNEL_IN_MONO;
        final int PENC = AudioFormat.ENCODING_PCM_16BIT;
        final int FR = 960;   // 20 ms @ 48k
        final int GAIN = 1;   // uplink passthrough — no boost (fixed boost clips normal-volume speech).
                              // Channels are separated, so balance in post (lossless) instead of gain here.
        AudioRecord up = null, down = null;
        try {
            int min = AudioRecord.getMinBufferSize(RATE, MONO, PENC);
            int buf = Math.max(min * 4, RATE);
            up = tryOpen(UP, RATE, MONO, PENC, buf, "VOICE_UPLINK(L)");
            down = tryOpen(DOWN, RATE, MONO, PENC, buf, "VOICE_DOWNLINK(R)");
            if (up == null && down == null) { Log.e(CallRecApp.T, "both sources failed"); return; }

            String base = CallRecApp.dir != null ? CallRecApp.dir : "/storage/emulated/0/Recordings";
            // Colons are illegal on Android's emulated storage (FAT-compat rule), so use dots for the time.
            String stamp = new java.text.SimpleDateFormat("yyyy-MM-dd HH.mm.ss", java.util.Locale.US).format(new java.util.Date());
            String path = base + "/call " + stamp + ".ogg";
            java.io.File pf = new java.io.File(path).getParentFile();
            if (pf != null) pf.mkdirs();

            MediaFormat fmt = MediaFormat.createAudioFormat("audio/opus", RATE, 2);
            fmt.setInteger(MediaFormat.KEY_BIT_RATE, 64000);
            fmt.setInteger(MediaFormat.KEY_PCM_ENCODING, PENC);
            fmt.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, FR * 4 * 2);
            enc = MediaCodec.createEncoderByType("audio/opus");
            enc.configure(fmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            enc.start();
            mux = new MediaMuxer(path, MediaMuxer.OutputFormat.MUXER_OUTPUT_OGG);

            if (up != null) up.startRecording();
            if (down != null) down.startRecording();

            short[] l = new short[FR], r = new short[FR];
            byte[] pcm = new byte[FR * 4];
            long frames = 0, sqL = 0, sqR = 0, cnt = 0, sinceLog = 0;
            while (!stop) {
                int nL = up != null ? up.read(l, 0, FR) : FR;
                if (up == null) Arrays.fill(l, (short) 0);
                int nR = down != null ? down.read(r, 0, FR) : FR;
                if (down == null) Arrays.fill(r, (short) 0);
                if (nL < 0 || nR < 0) { Log.e(CallRecApp.T, "read err L=" + nL + " R=" + nR); break; }
                int m = Math.min(nL, nR);
                int bi = 0;
                for (int i = 0; i < m; i++) {
                    int gl = l[i] * GAIN;
                    if (gl > 32767) gl = 32767; else if (gl < -32768) gl = -32768;
                    short sl = (short) gl, sr = r[i];
                    pcm[bi++] = (byte) (sl & 0xff); pcm[bi++] = (byte) ((sl >> 8) & 0xff);
                    pcm[bi++] = (byte) (sr & 0xff); pcm[bi++] = (byte) ((sr >> 8) & 0xff);
                    sqL += (long) sl * sl; sqR += (long) sr * sr; cnt++;
                }
                drain(false);
                feed(pcm, bi, frames * 1000000L / RATE, false);
                frames += m;
                sinceLog += m;
                if (sinceLog >= RATE * 5) {
                    double rmsL = cnt > 0 ? Math.sqrt((double) sqL / cnt) : 0;
                    double rmsR = cnt > 0 ? Math.sqrt((double) sqR / cnt) : 0;
                    Log.i(CallRecApp.T, "enc " + (frames / RATE) + "s rms L(you)=" + (int) rmsL + " R(them)=" + (int) rmsR);
                    sqL = 0; sqR = 0; cnt = 0; sinceLog = 0;
                }
            }
            feed(null, 0, frames * 1000000L / RATE, true);
            drain(true);
            if (up != null) up.stop();
            if (down != null) down.stop();
            Log.i(CallRecApp.T, "stopped, " + (frames / RATE) + "s -> " + path);
            try {
                if (CallRecApp.ctx != null)
                    android.media.MediaScannerConnection.scanFile(CallRecApp.ctx, new String[]{path}, new String[]{"audio/ogg"}, null);
            } catch (Throwable ignore) {}
        } catch (Throwable e) {
            Log.e(CallRecApp.T, "recorder err", e);
        } finally {
            try { if (up != null) up.release(); } catch (Throwable ignore) {}
            try { if (down != null) down.release(); } catch (Throwable ignore) {}
            try { if (enc != null) { enc.stop(); enc.release(); } } catch (Throwable ignore) {}
            try { if (mux != null) { if (muxStarted) mux.stop(); mux.release(); } } catch (Throwable ignore) {}
        }
    }

    private void feed(byte[] pcm, int len, long ptsUs, boolean end) {
        int ii = enc.dequeueInputBuffer(20000);
        if (ii < 0) { if (!end) Log.w(CallRecApp.T, "no input buffer, dropped " + len); return; }
        ByteBuffer ib = enc.getInputBuffer(ii);
        ib.clear();
        if (end) { enc.queueInputBuffer(ii, 0, 0, ptsUs, MediaCodec.BUFFER_FLAG_END_OF_STREAM); }
        else { ib.put(pcm, 0, len); enc.queueInputBuffer(ii, 0, len, ptsUs, 0); }
    }

    private void drain(boolean end) {
        while (true) {
            int oi = enc.dequeueOutputBuffer(info, end ? 10000 : 0);
            if (oi == MediaCodec.INFO_TRY_AGAIN_LATER) {
                if (!end) return; else continue;
            } else if (oi == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                if (!muxStarted) { track = mux.addTrack(enc.getOutputFormat()); mux.start(); muxStarted = true; }
            } else if (oi >= 0) {
                ByteBuffer ob = enc.getOutputBuffer(oi);
                if ((info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) info.size = 0;
                if (info.size > 0 && muxStarted) {
                    ob.position(info.offset); ob.limit(info.offset + info.size);
                    mux.writeSampleData(track, ob, info);
                }
                enc.releaseOutputBuffer(oi, false);
                if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) return;
            }
        }
    }

    static AudioRecord tryOpen(int src, int rate, int ch, int enc, int buf, String label) {
        try {
            AudioRecord ar = new AudioRecord(src, rate, ch, enc, buf);
            int st = ar.getState();
            Log.i(CallRecApp.T, "open " + label + " state=" + st + (st == AudioRecord.STATE_INITIALIZED ? " OK" : " FAILED"));
            if (st != AudioRecord.STATE_INITIALIZED) { ar.release(); return null; }
            return ar;
        } catch (Throwable e) { Log.e(CallRecApp.T, "open " + label + " threw", e); return null; }
    }
}
