package com.pocketguard;

/** Deferred re-check runnable posted after a screen-on. */
public class Recheck implements Runnable {
    private final String tag;
    public Recheck(String tag) { this.tag = tag; }
    @Override public void run() { PocketApp.recheck(tag); }
}
