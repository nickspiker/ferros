package com.pocketguard;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Fires when the screen turns on; hands off to PocketApp.onScreenOn(). */
public class ScreenReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context c, Intent i) { PocketApp.onScreenOn(); }
}
