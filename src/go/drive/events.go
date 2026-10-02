package drive

// Listing cache gated by Drive volume events.
//
// Proton asks third-party clients to follow the volume's event log instead of re-listing the tree on a timer. Every sync pass still asks for a full recursive listing, so the Session answers those from memory as long as the event log reports no change since the listing was taken. One cheap events request per pass replaces a ListChildren call per folder; the tree is only walked again after something actually changed (including Celeste's own uploads and deletes, which invalidate the cache directly).

import (
	"context"
	"log"
	"sync"
	"time"

	"celeste/native-go/proton-ext"
)

const (
	// eventPollGap bounds how often the event log is asked. A sync pass resolves several paths and lists once; they all share one poll.
	eventPollGap = 5 * time.Second
	// listingMaxAge forces a fresh walk now and then, in case an event was missed.
	listingMaxAge = time.Hour
	// maxEventPages caps one poll; a longer backlog simply invalidates the cache, which is what any non-empty page does anyway.
	maxEventPages = 20
)

type listingCache struct {
	mu       sync.Mutex
	cursor   string // last event ID seen; empty until the first poll
	polledAt time.Time
	filledAt time.Time
	dirs     map[string][]*Entry // folder link ID → active children
	trees    map[string][]*Entry // root link ID → recursive listing
}

// clear drops every cached listing. Caller holds mu.
func (l *listingCache) clear() {
	l.dirs = nil
	l.trees = nil
	l.filledAt = time.Time{}
}

// invalidateListings forgets all cached listings. Called after every mutation Celeste makes itself, so the rest of the pass sees its own changes without waiting for the event.
func (s *Session) invalidateListings() {
	s.listings.mu.Lock()
	defer s.listings.mu.Unlock()
	s.listings.clear()
}

// pollEvents brings the cache up to date with the volume's event log. Any event, a refresh request from the server, or an error empties the cache; the caller then lists from the API. Caller holds mu.
func (s *Session) pollEvents(ctx context.Context) {
	l := &s.listings
	if time.Since(l.polledAt) < eventPollGap {
		return
	}
	l.polledAt = time.Now()
	if !l.filledAt.IsZero() && time.Since(l.filledAt) > listingMaxAge {
		l.clear()
	}

	auth := s.protonextAuth()
	if l.cursor == "" {
		id, err := protonext.LatestVolumeEventID(ctx, auth, s.volumeID)
		if err != nil {
			log.Printf("drive: latest volume event: %v", err)
			l.clear()
			return
		}
		// Nothing is cached before the first cursor, so there is nothing to invalidate.
		l.cursor = id
		return
	}

	for range maxEventPages {
		page, err := protonext.VolumeEvents(ctx, auth, s.volumeID, l.cursor)
		if err != nil {
			// An expired or unknown cursor also lands here; start over from the latest event.
			log.Printf("drive: volume events: %v", err)
			l.clear()
			l.cursor = ""
			return
		}
		if page.Refresh || len(page.Events) > 0 {
			l.clear()
		}
		if page.EventID != "" {
			l.cursor = page.EventID
		}
		if !page.More {
			return
		}
	}
	l.clear()
}

// cachedListing returns the cached listing of `linkID` if the event log shows no change since it was stored.
func (s *Session) cachedListing(ctx context.Context, recursive bool, linkID string) ([]*Entry, bool) {
	s.listings.mu.Lock()
	defer s.listings.mu.Unlock()
	s.pollEvents(ctx)
	m := s.listings.dirs
	if recursive {
		m = s.listings.trees
	}
	out, ok := m[linkID]
	return out, ok
}

// storeListing caches a fresh listing. A change landing between the poll and the listing shows up as an event on the next poll and drops it again.
func (s *Session) storeListing(recursive bool, linkID string, entries []*Entry) {
	l := &s.listings
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.cursor == "" {
		// No event cursor yet (the events request failed): there is no way to notice later changes, so don't cache.
		return
	}
	if l.filledAt.IsZero() {
		l.filledAt = time.Now()
	}
	if recursive {
		if l.trees == nil {
			l.trees = make(map[string][]*Entry)
		}
		l.trees[linkID] = entries
	} else {
		if l.dirs == nil {
			l.dirs = make(map[string][]*Entry)
		}
		l.dirs[linkID] = entries
	}
}
