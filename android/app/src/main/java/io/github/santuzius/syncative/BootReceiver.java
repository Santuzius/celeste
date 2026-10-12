package io.github.santuzius.syncative;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

import java.io.File;

/** Starts syncing after a reboot or an update of Syncative, unless the user turned off "Run in background" or "Start when the device starts" in the preferences. */
public class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        // The switch src/services/autostart_android.rs writes: $XDG_DATA_HOME/syncative/autostart-off.
        if (SyncService.backgroundOff(context) || new File(context.getFilesDir(), "syncative/autostart-off").exists()) {
            return;
        }
        SyncService.start(context);
    }
}
