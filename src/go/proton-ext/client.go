package protonext

// HTTP wrapper for the handful of Drive endpoints upstream
// go-proton-api doesn't expose. We can't call upstream's `Client.do()`
// from outside the package (it's unexported), and we can't add methods
// to `*proton.Client` from here either, so we run a parallel HTTP path
// (stdlib net/http, no resty) whose auth headers are populated from
// the same UID/access token Celeste already has on its own Session.
//
// Trade-off: this client doesn't share upstream's auto-refresh on 401,
// status observers, or per-error-code handlers. For revision-lifecycle
// calls inside a single sync run that's acceptable — if the access
// token expires mid-call the user re-runs and a fresh Login() seeds a
// new token.

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/ProtonMail/go-proton-api"
)

// Auth carries the credential bits a request needs. Populate from the
// celeste/go/drive Session's exposed fields. HostURL is optional;
// when blank we fall back to upstream's `proton.DefaultHostURL` so this
// package and upstream stay pinned to the same endpoint.
type Auth struct {
	UID         string
	AccessToken string
	AppVersion  string
	HostURL     string // empty → proton.DefaultHostURL
}

func (a Auth) hostURL() string {
	if a.HostURL != "" {
		return a.HostURL
	}
	return proton.DefaultHostURL
}

// httpClient is shared across calls — single transport + connection
// pool. Threaded through context-bound requests so cancellation and
// per-call timeouts behave the way callers expect.
var (
	httpClientOnce sync.Once
	httpClient     *http.Client
)

func sharedHTTPClient() *http.Client {
	httpClientOnce.Do(func() {
		httpClient = &http.Client{Timeout: 90 * time.Second}
	})
	return httpClient
}

// do performs an authenticated request and decodes a JSON response into
// `out` (pass nil when the response body is irrelevant). `body` is
// JSON-encoded when non-nil. On API failure the returned error is
// `*proton.APIError` so callers can use `errors.As(err,
// **proton.APIError)` the same way they do for upstream's calls.
func do(ctx context.Context, auth Auth, method, path string, body, out any) error {
	url := strings.TrimRight(auth.hostURL(), "/") + path

	var bodyReader io.Reader
	if body != nil {
		buf, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("encode body: %w", err)
		}
		bodyReader = bytes.NewReader(buf)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	req.Header.Set("Accept", "application/json")
	if auth.AppVersion != "" {
		req.Header.Set("x-pm-appversion", auth.AppVersion)
	}
	if auth.UID != "" {
		req.Header.Set("x-pm-uid", auth.UID)
	}
	if auth.AccessToken != "" {
		req.Header.Set("Authorization", "Bearer "+auth.AccessToken)
	}

	res, err := sharedHTTPClient().Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()

	respBody, err := io.ReadAll(res.Body)
	if err != nil {
		return err
	}

	if res.StatusCode >= 400 {
		ae := &proton.APIError{Status: res.StatusCode}
		// Upstream's APIError has a JSON tag mapping `Error` →
		// Message, so the standard unmarshal populates it correctly.
		if jerr := json.Unmarshal(respBody, ae); jerr != nil {
			return fmt.Errorf("HTTP %d: %s", res.StatusCode, strings.TrimSpace(string(respBody)))
		}
		return ae
	}

	if out != nil {
		if err := json.Unmarshal(respBody, out); err != nil {
			return fmt.Errorf("decode response: %w", err)
		}
	}
	return nil
}
