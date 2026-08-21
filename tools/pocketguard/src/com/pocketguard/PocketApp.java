package com.pocketguard;

import android.app.Application;
import android.content.Context;
import android.content.IntentFilter;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;
import android.os.SystemClock;
import android.util.Log;
import java.io.BufferedReader;
import java.io.InputStreamReader;

/**
 * Dumb-simple pocket guard: watch the RAW proximity sensor (TMD3719, not the fused
 * on-ear one the display uses). If anything is near the sensor and the screen comes on
 * (or is on), force it back to sleep. No orientation guessing: near == off, period.
 *
 * State is static so the top-level ScreenReceiver / Recheck helpers can reach it without
 * inner classes — the bundled d8 NPEs on inner classes here.
 */
public class PocketApp extends Application implements SensorEventListener {
    static final String T = "PG";
    static volatile boolean near = false;
    static volatile float lastVal = -1f;
    static volatile long lastEventMs = 0;
    static float maxRange = 5f;
    static PowerManager pm;
    static Handler handler;

    @Override public void onCreate() {
        super.onCreate();
        try {
            handler = new Handler(Looper.getMainLooper());
            pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
            SensorManager sm = (SensorManager) getSystemService(Context.SENSOR_SERVICE);
            Sensor prox = sm.getDefaultSensor(Sensor.TYPE_PROXIMITY);
            if (prox != null) {
                maxRange = prox.getMaximumRange();
                boolean ok = sm.registerListener(this, prox, SensorManager.SENSOR_DELAY_FASTEST);
                Log.i(T, "onCreate bound prox name=" + prox.getName() + " vendor=" + prox.getVendor()
                        + " type=" + prox.getType() + " maxRange=" + maxRange + " register=" + ok);
            } else {
                Log.e(T, "onCreate NO default proximity sensor");
            }
            registerReceiver(new ScreenReceiver(), new IntentFilter(android.content.Intent.ACTION_SCREEN_ON));
            Log.i(T, "onCreate done");
        } catch (Throwable t) { Log.e(T, "init failed", t); }
    }

    @Override public void onSensorChanged(SensorEvent e) {
        lastVal = e.values[0];
        lastEventMs = SystemClock.elapsedRealtime();
        near = lastVal < maxRange;
        boolean on = pm != null && pm.isScreenOn();
        Log.i(T, "prox val=" + lastVal + " near=" + near + " screenOn=" + on);
        if (near && on) sleepScreen("sensor");
    }
    @Override public void onAccuracyChanged(Sensor s, int a) {}

    /** Called by ScreenReceiver when the screen turns on. */
    static void onScreenOn() {
        long age = SystemClock.elapsedRealtime() - lastEventMs;
        Log.i(T, "SCREEN_ON near=" + near + " lastVal=" + lastVal + " ageMs=" + age);
        if (near) sleepScreen("screen_on");
        if (handler != null) {
            handler.postDelayed(new Recheck("recheck80"), 80);
            handler.postDelayed(new Recheck("recheck220"), 220);
        }
    }

    /** Deferred re-check from Recheck: prox reading can lag the wake by a few ms. */
    static void recheck(String tag) {
        Log.i(T, tag + " near=" + near + " lastVal=" + lastVal);
        if (near) sleepScreen(tag);
    }

    static void sleepScreen(String why) {
        try {
            Log.i(T, "sleepScreen(" + why + ") exec su input keyevent 223");
            Process p = Runtime.getRuntime().exec(new String[]{"su", "-c", "input keyevent 223"});
            int code = p.waitFor();
            String err = read(p.getErrorStream());
            Log.i(T, "sleepScreen(" + why + ") exit=" + code + (err.isEmpty() ? "" : " err=" + err));
        } catch (Throwable t) { Log.e(T, "sleep failed", t); }
    }

    static String read(java.io.InputStream is) {
        try {
            BufferedReader r = new BufferedReader(new InputStreamReader(is));
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = r.readLine()) != null) sb.append(line).append(' ');
            return sb.toString().trim();
        } catch (Throwable t) { return ""; }
    }
}
