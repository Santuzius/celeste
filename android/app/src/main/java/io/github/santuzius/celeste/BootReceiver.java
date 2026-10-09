package io.github.santuzius.celeste;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

import java.io.File;

/** Starts syncing after a reboot or an update of Celeste, unless the user turned off "Start at login" in the preferences. */
public class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        // The switch src/services/autostart_android.rs writes: $XDG_DATA_HOME/celeste/autostart-off.
        if (new File(context.getFilesDir(), "celeste/autostart-off").exists()) {
            return;
        }
        SyncService.start(context);
    }
}
