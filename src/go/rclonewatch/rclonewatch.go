// Package rclonewatch tracks remote-side changes of rclone remotes through the backend's change notification (Google Drive's and Dropbox's change logs), so Celeste can reuse a remote listing until something actually changed.
package rclonewatch

import (
	"context"
	"sync"
	"sync/atomic"
	"time"

	"github.com/rclone/rclone/fs"
	"github.com/rclone/rclone/fs/cache"
)

const (
	// defaultPollInterval is how often the backend asks the provider's change log; one cheap request per remote.
	defaultPollInterval = 5 * time.Second
	// startupGrace reports every remote as changed for a while after watching starts: the backend fetches its first change-log cursor asynchronously, and changes before that cursor would go unnoticed.
	startupGrace = 3 * defaultPollInterval
)

type watcher struct {
	started   time.Time
	dirty     atomic.Bool
	intervals chan time.Duration // closing it stops the backend's poll loop
}

var (
	mu       sync.Mutex
	watchers = map[string]*watcher{} // nil value: backend without change notification
	// pollInterval is the current one; SetPollInterval changes it.
	pollInterval = defaultPollInterval
)

// SetPollInterval changes how often every watched remote, and every one watched from now on, asks its change log, e.g. less often while the device sleeps. Zero restores the default.
func SetPollInterval(d time.Duration) {
	if d <= 0 {
		d = defaultPollInterval
	}
	mu.Lock()
	defer mu.Unlock()
	pollInterval = d
	for _, w := range watchers {
		if w == nil {
			continue
		}
		// The backend reads the channel all the time; replace a value it has not picked up yet.
		select {
		case <-w.intervals:
		default:
		}
		select {
		case w.intervals <- d:
		default:
		}
	}
}

// Changed reports whether `remote` may have changed since the previous call. The first call starts watching the remote; remotes whose backend has no change notification always report a change.
func Changed(remote string) bool {
	mu.Lock()
	defer mu.Unlock()
	if w, ok := watchers[remote]; ok {
		if w == nil {
			return true
		}
		dirty := w.dirty.Swap(false)
		return dirty || time.Since(w.started) < startupGrace
	}

	f, err := cache.Get(context.Background(), remote+":")
	if err != nil {
		// Not recorded: try again on the next call.
		return true
	}
	notify := f.Features().ChangeNotify
	if notify == nil {
		watchers[remote] = nil
		return true
	}
	w := &watcher{started: time.Now(), intervals: make(chan time.Duration, 1)}
	w.intervals <- pollInterval
	notify(context.Background(), func(string, fs.EntryType) { w.dirty.Store(true) }, w.intervals)
	watchers[remote] = w
	return true
}

// Forget stops watching `remote`, e.g. after its config was replaced or deleted. The next Changed call starts over.
func Forget(remote string) {
	mu.Lock()
	defer mu.Unlock()
	if w := watchers[remote]; w != nil {
		close(w.intervals)
	}
	delete(watchers, remote)
}
