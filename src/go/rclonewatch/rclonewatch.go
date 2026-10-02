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
	// pollInterval is how often the backend asks the provider's change log; one cheap request per remote.
	pollInterval = 5 * time.Second
	// startupGrace reports every remote as changed for a while after watching starts: the backend fetches its first change-log cursor asynchronously, and changes before that cursor would go unnoticed.
	startupGrace = 3 * pollInterval
)

type watcher struct {
	started   time.Time
	dirty     atomic.Bool
	intervals chan time.Duration // closing it stops the backend's poll loop
}

var (
	mu       sync.Mutex
	watchers = map[string]*watcher{} // nil value: backend without change notification
)

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
