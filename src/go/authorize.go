package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"encoding/json"
	"net/http"
	"sync"
	"time"

	"github.com/rclone/rclone/fs"
	"github.com/rclone/rclone/fs/config"
	"github.com/rclone/rclone/fs/config/configmap"
	"github.com/rclone/rclone/lib/oauthutil"
)

// What `rclone authorize` does, inside Celeste: the rclone binary is not there on Android, and a separate process is not needed elsewhere either. Instead of opening a browser itself rclone hands the authorization link over (see CelesteAuthorizeURL), so Celeste can open it in the platform's way.

var (
	authorizeURLMutex sync.Mutex
	authorizeURL      string
)

func init() {
	oauthutil.OpenURL = func(url string) error {
		authorizeURLMutex.Lock()
		authorizeURL = url
		authorizeURLMutex.Unlock()
		return nil
	}
}

// CelesteAuthorize runs rclone's OAuth flow for the backend `typ` ("drive", "dropbox", "pcloud"), with the given client ID and secret unless empty, and returns `{"token": "…"}` or `{"error": "…"}`. Blocks until the browser returns to rclone's local web server, or CelesteAuthorizeCancel. Free the result with RcloneFreeString.
//
//export CelesteAuthorize
func CelesteAuthorize(typ, clientID, clientSecret *C.char) *C.char {
	// Cleared on the way out as well, so the next run never sees this run's link.
	clearURL := func() {
		authorizeURLMutex.Lock()
		authorizeURL = ""
		authorizeURLMutex.Unlock()
	}
	clearURL()
	defer clearURL()

	reply := map[string]string{}
	if token, err := authorize(C.GoString(typ), C.GoString(clientID), C.GoString(clientSecret)); err != nil {
		reply["error"] = err.Error()
	} else {
		reply["token"] = token
	}
	out, _ := json.Marshal(reply)
	return C.CString(string(out))
}

// CelesteAuthorizeURL returns the authorization link of the running CelesteAuthorize, or NULL before rclone has made it. Free it with RcloneFreeString.
//
//export CelesteAuthorizeURL
func CelesteAuthorizeURL() *C.char {
	authorizeURLMutex.Lock()
	defer authorizeURLMutex.Unlock()
	if authorizeURL == "" {
		return nil
	}
	return C.CString(authorizeURL)
}

// CelesteAuthorizeCancel makes a running CelesteAuthorize return with an error. rclone waits for its web server without a timeout, so this calls the server the way the browser would, only without a code.
//
//export CelesteAuthorizeCancel
func CelesteAuthorizeCancel() {
	client := http.Client{Timeout: 2 * time.Second}
	if resp, err := client.Get(oauthutil.RedirectURL); err == nil {
		resp.Body.Close()
	}
}

// authorize mirrors config.Authorize with a client ID and secret, but returns the token instead of printing it.
func authorize(typ, clientID, clientSecret string) (string, error) {
	ctx, ci := fs.AddConfig(context.Background())
	ci.AutoConfirm = true
	ctx = fs.ConfigOAuthOnly(ctx)
	ri, err := fs.Find(typ)
	if err != nil {
		return "", err
	}

	in := configmap.Simple{config.ConfigAuthorize: "true"}
	if clientID != "" && clientSecret != "" {
		in[config.ConfigClientID] = clientID
		in[config.ConfigClientSecret] = clientSecret
	}
	const name = "**temp-fs**"
	m := fs.ConfigMap(ri.Prefix, ri.Options, name, in)
	out := configmap.Simple{}
	m.ClearSetters()
	m.AddSetter(out)
	m.AddGetter(out, configmap.PriorityNormal)
	if err := config.PostConfig(ctx, name, m, ri); err != nil {
		return "", err
	}
	return out["token"], nil
}
