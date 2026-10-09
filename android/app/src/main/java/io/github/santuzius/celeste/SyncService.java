package io.github.santuzius.celeste;

import android.app.Notification;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import android.util.Log;

/** Keeps the process, and with it the sync engine, alive while Celeste is not on screen. Its silent notification shows the sync status. Started by the engine itself and after a reboot. */
public class SyncService extends Service {
    private static final int NOTIFICATION_ID = 1;

    private static volatile SyncService running;
    private static volatile String status = "Starting…";

    /** Starts the sync engine without a GUI, unless it already runs. */
    static native void nativeStartEngine();

    /** Shows `text` as the sync status, starting the service if needed. */
    static void show(Context context, String text) {
        status = text;
        SyncService service = running;
        if (service != null) {
            context.getSystemService(NotificationManager.class).notify(NOTIFICATION_ID, service.notification());
        } else {
            start(context);
        }
    }

    static void start(Context context) {
        try {
            context.startForegroundService(new Intent(context, SyncService.class));
        } catch (IllegalStateException e) {
            // Android 12+ refuses while the app is in the background, apart from exemptions such as boot; the next status change from the foreground starts it.
            Log.w(Bridge.TAG, "could not start the sync service", e);
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        running = this;
        nativeStartEngine();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Notification notification = notification();
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(NOTIFICATION_ID, notification);
        }
        // Should Android stop the process, it restarts the service, and with it syncing.
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        running = null;
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    private Notification notification() {
        Bridge.createChannels(this);
        return new Notification.Builder(this, Bridge.CHANNEL_SYNC)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(status)
                .setContentIntent(Bridge.openApp(this))
                .setOngoing(true)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_SERVICE)
                .build();
    }
}
