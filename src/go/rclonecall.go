package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/rclone/rclone/fs/rc"
	"github.com/rclone/rclone/fs/rc/jobs"
)

// cancelPollInterval is how often a running call looks at its cancel flag.
const cancelPollInterval = 100 * time.Millisecond

// rpcWithCancel follows librclone.RPC, with a context that a set cancel flag cancels.
func rpcWithCancel(method string, input string, cancelled *uint8) (output string, status int) {
	in := make(rc.Params)
	defer func() {
		if r := recover(); r != nil {
			output, status = rpcError(method, in, fmt.Errorf("panic: %v", r), http.StatusInternalServerError)
		}
	}()
	if input != "" {
		if err := json.NewDecoder(strings.NewReader(input)).Decode(&in); err != nil {
			return rpcError(method, in, fmt.Errorf("failed to read input JSON: %w", err), http.StatusBadRequest)
		}
	}
	call := rc.Calls.Get(method)
	if call == nil || call.NeedsRequest || call.NeedsResponse {
		return rpcError(method, in, fmt.Errorf("method %q not supported", method), http.StatusNotFound)
	}

	ctx, stop := cancelledBy(cancelled)
	defer stop()

	_, out, err := jobs.NewJob(ctx, call.Fn, in)
	if err != nil {
		if errors.Is(ctx.Err(), context.Canceled) {
			err = fmt.Errorf("cancelled: %w", err)
		}
		return rpcError(method, in, err, http.StatusInternalServerError)
	}
	if out == nil {
		out = make(rc.Params)
	}
	var w strings.Builder
	if err := rc.WriteJSON(&w, out); err != nil {
		return rpcError(method, in, err, http.StatusInternalServerError)
	}
	return w.String(), http.StatusOK
}

// cancelledBy returns a context that is cancelled once the byte at `flag` turns non-zero. Call `stop` when done with it.
func cancelledBy(flag *uint8) (ctx context.Context, stop func()) {
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() {
		ticker := time.NewTicker(cancelPollInterval)
		defer ticker.Stop()
		for {
			select {
			case <-done:
				return
			case <-ticker.C:
				// Written atomically by Rust; a byte read is never torn.
				if *flag != 0 {
					cancel()
					return
				}
			}
		}
	}()
	return ctx, func() {
		close(done)
		cancel()
	}
}

// rpcError is the error JSON librclone.RPC returns.
func rpcError(path string, in rc.Params, err error, status int) (string, int) {
	params, status := rc.Error(path, in, err, status)
	var w strings.Builder
	if rc.WriteJSON(&w, params) != nil {
		b, _ := json.Marshal(map[string]any{"error": err.Error(), "path": path, "status": status})
		return string(b), status
	}
	return w.String(), status
}
