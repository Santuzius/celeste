package io.github.santuzius.syncative;

import android.app.Application;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Log;

import java.io.File;

/** Runs first in every Syncative process, whether the launcher, the sync service or the boot receiver started it: sets up the environment and loads the native library. */
public class SyncativeApplication extends Application {
    static native void nativeInit(Application application);

    @Override
    public void onCreate() {
        super.onCreate();

        File files = getFilesDir();
        File cache = new File(files, "cache");
        File run = new File(files, "run");
        cache.mkdirs();
        run.mkdirs();

        // Syncative, rclone and Go look for their directories here, and Android sets none of them. Set before the library loads: the Go runtime copies the environment when it starts.
        try {
            Os.setenv("HOME", files.getPath(), true);
            Os.setenv("XDG_DATA_HOME", files.getPath(), true);
            Os.setenv("XDG_CONFIG_HOME", files.getPath(), true);
            Os.setenv("XDG_CACHE_HOME", cache.getPath(), true);
            Os.setenv("XDG_RUNTIME_DIR", run.getPath(), true);
            // Go's os.TempDir falls back to /data/local/tmp, which apps cannot write.
            Os.setenv("TMPDIR", cache.getPath(), true);
        } catch (ErrnoException e) {
            Log.e("syncative", "setenv failed", e);
        }

        System.loadLibrary("syncative");
        nativeInit(this);
        Bridge.watchNetwork(this);
    }
}
