// Celeste's combined Go archive. Exposes both rclone's librclone RPC
// surface (replacement for the upstream librclone-sys crate) and our
// native ProtonDrive client's C entry points.
//
// Built as:
//
//	go build --buildmode=c-archive -o libceleste_native.a .
//
// One Go runtime, both feature sets. See the integration plan for
// why we can't have two independent Go archives coexisting in the
// Celeste binary.
package main

/*
#include <stdlib.h>

struct RcloneRPCResult {
	char*	Output;
	int	Status;
};
*/
import "C"

import (
	"context"
	"encoding/json"
	"reflect"
	"unsafe"

	"celeste/native-go/drive"

	"github.com/rclone/rclone/librclone/librclone"

	// Pull in the same rclone surface librclone-sys does, MINUS the
	// protondrive backend. Rclone's protondrive backend links in
	// henrybear327/go-proton-api (a fork pinned to an older resty
	// API) which collides with ProtonMail/go-proton-api + Proton's
	// resty fork that our native Drive layer needs. We own the
	// ProtonDrive path via native-go/drive/ (arriving in Phase 2+)
	// so dropping rclone's protondrive costs us nothing.
	//
	// This is an explicit expansion of github.com/rclone/rclone/backend/all
	// with the one import excluded. Keep in sync with upstream
	// /backend/all/all.go if we bump rclone.
	_ "github.com/rclone/rclone/backend/alias"
	_ "github.com/rclone/rclone/backend/azureblob"
	_ "github.com/rclone/rclone/backend/azurefiles"
	_ "github.com/rclone/rclone/backend/b2"
	_ "github.com/rclone/rclone/backend/box"
	_ "github.com/rclone/rclone/backend/cache"
	_ "github.com/rclone/rclone/backend/chunker"
	_ "github.com/rclone/rclone/backend/cloudinary"
	_ "github.com/rclone/rclone/backend/combine"
	_ "github.com/rclone/rclone/backend/compress"
	_ "github.com/rclone/rclone/backend/crypt"
	_ "github.com/rclone/rclone/backend/drive"
	_ "github.com/rclone/rclone/backend/dropbox"
	_ "github.com/rclone/rclone/backend/fichier"
	_ "github.com/rclone/rclone/backend/filefabric"
	_ "github.com/rclone/rclone/backend/filescom"
	_ "github.com/rclone/rclone/backend/ftp"
	_ "github.com/rclone/rclone/backend/gofile"
	_ "github.com/rclone/rclone/backend/googlecloudstorage"
	_ "github.com/rclone/rclone/backend/googlephotos"
	_ "github.com/rclone/rclone/backend/hasher"
	_ "github.com/rclone/rclone/backend/hdfs"
	_ "github.com/rclone/rclone/backend/hidrive"
	_ "github.com/rclone/rclone/backend/http"
	_ "github.com/rclone/rclone/backend/iclouddrive"
	_ "github.com/rclone/rclone/backend/imagekit"
	_ "github.com/rclone/rclone/backend/internetarchive"
	_ "github.com/rclone/rclone/backend/jottacloud"
	_ "github.com/rclone/rclone/backend/koofr"
	_ "github.com/rclone/rclone/backend/linkbox"
	_ "github.com/rclone/rclone/backend/local"
	_ "github.com/rclone/rclone/backend/mailru"
	_ "github.com/rclone/rclone/backend/mega"
	_ "github.com/rclone/rclone/backend/memory"
	_ "github.com/rclone/rclone/backend/netstorage"
	_ "github.com/rclone/rclone/backend/onedrive"
	_ "github.com/rclone/rclone/backend/opendrive"
	_ "github.com/rclone/rclone/backend/oracleobjectstorage"
	_ "github.com/rclone/rclone/backend/pcloud"
	_ "github.com/rclone/rclone/backend/pikpak"
	_ "github.com/rclone/rclone/backend/pixeldrain"
	_ "github.com/rclone/rclone/backend/premiumizeme"
	// protondrive intentionally omitted — replaced by native Go layer
	_ "github.com/rclone/rclone/backend/putio"
	_ "github.com/rclone/rclone/backend/qingstor"
	_ "github.com/rclone/rclone/backend/quatrix"
	_ "github.com/rclone/rclone/backend/s3"
	_ "github.com/rclone/rclone/backend/seafile"
	_ "github.com/rclone/rclone/backend/sftp"
	_ "github.com/rclone/rclone/backend/sharefile"
	_ "github.com/rclone/rclone/backend/sia"
	_ "github.com/rclone/rclone/backend/smb"
	_ "github.com/rclone/rclone/backend/storj"
	_ "github.com/rclone/rclone/backend/sugarsync"
	_ "github.com/rclone/rclone/backend/swift"
	_ "github.com/rclone/rclone/backend/ulozto"
	_ "github.com/rclone/rclone/backend/union"
	_ "github.com/rclone/rclone/backend/uptobox"
	_ "github.com/rclone/rclone/backend/webdav"
	_ "github.com/rclone/rclone/backend/yandex"
	_ "github.com/rclone/rclone/backend/zoho"

	_ "github.com/rclone/rclone/cmd/cmount"
	_ "github.com/rclone/rclone/cmd/mount"
	_ "github.com/rclone/rclone/cmd/mount2"
	_ "github.com/rclone/rclone/fs/operations"
	_ "github.com/rclone/rclone/fs/sync"
	_ "github.com/rclone/rclone/lib/plugin"

	// Our own Drive layer (native-go/drive/) imports go-proton-api
	// directly. Proton-API-Bridge was removed in Phase 7 after the
	// port was validated — the only remaining ties to it are attribution
	// comments in files that started as ports of its code.
	"github.com/ProtonMail/go-proton-api"
)

// ---------------- librclone ABI ----------------
// These functions mirror upstream librclone-sys's C surface so any
// existing Rust caller (celeste::infrastructure::rclone) keeps working
// unchanged after we swap the crate dep.

//export RcloneInitialize
func RcloneInitialize() {
	librclone.Initialize()
}

//export RcloneFinalize
func RcloneFinalize() {
	librclone.Finalize()
}

// RcloneRPCResult mirrors the struct librclone-sys consumes. Must stay
// layout-compatible; the Rust side pattern-matches on it.
type RcloneRPCResult struct { //nolint:deadcode,unused
	Output *C.char
	Status C.int
}

//export RcloneRPC
func RcloneRPC(method *C.char, input *C.char) (result C.struct_RcloneRPCResult) {
	output, status := librclone.RPC(C.GoString(method), C.GoString(input))
	result.Output = C.CString(output)
	result.Status = C.int(status)
	return result
}

//export RcloneFreeString
func RcloneFreeString(str *C.char) {
	C.free(unsafe.Pointer(str))
}

// ---------------- ProtonDrive native surface ----------------
//
// Calls are shaped as "takes one JSON string, returns one JSON string".
// The Rust side marshals arguments into a small request struct and
// reads the return value as a result envelope — see
// `native-go/src/lib.rs`. Caller is responsible for freeing every
// returned *C.char with RcloneFreeString (same Go allocator).
//
// Error handling: any result that's not an outright success returns
// a JSON object with an `error` field describing what went wrong.
// Crashes in Go are converted to error strings — the FFI boundary
// never panics.

// result is the envelope every ProtonDrive_* export returns as JSON.
type result struct {
	OK    bool            `json:"ok"`
	Data  json.RawMessage `json:"data,omitempty"`
	Error string          `json:"error,omitempty"`
}

// okResult returns a JSON success envelope; `data` can be nil.
//
// Typed-nil collections (a Go gotcha: a non-nil interface{} wrapping a
// nil slice/map) are normalised to "[]" / "{}" before marshalling. The
// Rust side parses Envelope.data as Option<serde_json::Value>, where a
// JSON `null` deserialises to None — same as a missing field — and
// any caller using `call_json::<_, Vec<T>>` then bails with "shim
// returned OK but no data to deserialise". Emitting an empty
// collection instead keeps that path honest.
func okResult(data interface{}) *C.char {
	envelope := result{OK: true}
	if data != nil {
		v := reflect.ValueOf(data)
		switch v.Kind() {
		case reflect.Slice:
			if v.IsNil() {
				envelope.Data = []byte("[]")
			} else {
				b, err := json.Marshal(data)
				if err != nil {
					return errResult(err)
				}
				envelope.Data = b
			}
		case reflect.Map:
			if v.IsNil() {
				envelope.Data = []byte("{}")
			} else {
				b, err := json.Marshal(data)
				if err != nil {
					return errResult(err)
				}
				envelope.Data = b
			}
		default:
			b, err := json.Marshal(data)
			if err != nil {
				return errResult(err)
			}
			envelope.Data = b
		}
	}
	b, _ := json.Marshal(envelope)
	return C.CString(string(b))
}

// errResult converts a Go error into the JSON error envelope the Rust
// side consumes.
func errResult(err error) *C.char {
	envelope := result{OK: false, Error: err.Error()}
	b, _ := json.Marshal(envelope)
	return C.CString(string(b))
}

// ProtonDrive_Version returns a short identity string proving the
// go-proton-api dependency linked into our archive. Caller must free
// the result with RcloneFreeString (same allocator).
//
//export ProtonDrive_Version
func ProtonDrive_Version() *C.char {
	// Build a fresh Manager just to exercise the symbol path; we throw
	// it away immediately. No network traffic.
	_ = proton.New(proton.WithAppVersion(drive.AppVersion))
	return C.CString("celeste-native proton-api bound")
}

// ProtonDrive_Login performs a full username+password (+ optional
// TOTP / mailbox password) login. Input JSON follows
// `drive.LoginParams`; on success the result data is a
// `drive.ReusableCredential` (UID, tokens, salted key pass). The
// session is registered in-process so subsequent calls can reference
// it by UID.
//
//export ProtonDrive_Login
func ProtonDrive_Login(paramsJSON *C.char) *C.char {
	var p drive.LoginParams
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Login(context.Background(), p)
	if err != nil {
		return errResult(err)
	}
	drive.Register(sess)
	return okResult(sess.AsCredential())
}

// ProtonDrive_Logout revokes the session on Proton's side and tears
// down the local state. Input JSON is `{"uid": "..."}`.
//
//export ProtonDrive_Logout
func ProtonDrive_Logout(paramsJSON *C.char) *C.char {
	var p struct {
		UID string `json:"uid"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	err = sess.Logout(context.Background())
	drive.Unregister(p.UID)
	if err != nil {
		return errResult(err)
	}
	return okResult(nil)
}

// ProtonDrive_SaveSession serializes the named session's reusable
// credential to `path`. Input JSON is `{"uid":"...","path":"..."}`.
//
//export ProtonDrive_SaveSession
func ProtonDrive_SaveSession(paramsJSON *C.char) *C.char {
	var p struct {
		UID  string `json:"uid"`
		Path string `json:"path"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	if err := sess.Save(p.Path); err != nil {
		return errResult(err)
	}
	return okResult(nil)
}

// ProtonDrive_SaveSessionIfRotated writes the session blob to `path`
// only when the access/refresh tokens have rotated since the last
// save, so the caller can cheaply poll after each sync pass and skip
// the keyring write when nothing changed. Input JSON is
// `{"uid":"...","path":"..."}`; result data is `{"saved": bool}`.
//
//export ProtonDrive_SaveSessionIfRotated
func ProtonDrive_SaveSessionIfRotated(paramsJSON *C.char) *C.char {
	var p struct {
		UID  string `json:"uid"`
		Path string `json:"path"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	saved, err := sess.SaveIfRotated(p.Path)
	if err != nil {
		return errResult(err)
	}
	return okResult(struct {
		Saved bool `json:"saved"`
	}{Saved: saved})
}

// ProtonDrive_ResumeSession reads a saved credential blob from disk
// and rehydrates a session. Input JSON is `{"path":"..."}`; result
// data is the `drive.ReusableCredential` of the resumed session so
// the caller can pick up the UID.
//
//export ProtonDrive_ResumeSession
func ProtonDrive_ResumeSession(paramsJSON *C.char) *C.char {
	var p struct {
		Path string `json:"path"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	cred, err := drive.LoadCredential(p.Path)
	if err != nil {
		return errResult(err)
	}
	sess, err := drive.Resume(context.Background(), cred)
	if err != nil {
		return errResult(err)
	}
	drive.Register(sess)
	return okResult(sess.AsCredential())
}

// ProtonDrive_RootLinkID returns the active-volume root link's ID for
// the named session. Input JSON `{"uid":"..."}`; result data is the
// string link ID. Handy for callers that want to start their
// traversal at the root without hard-coding a separate call.
//
//export ProtonDrive_RootLinkID
func ProtonDrive_RootLinkID(paramsJSON *C.char) *C.char {
	var p struct {
		UID string `json:"uid"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	return okResult(sess.RootLinkID())
}

// ProtonDrive_ListDirectory lists the active children of a folder.
// Input `{"uid":"...","link_id":"..."}`; `link_id` empty means list
// the root. Result data is a JSON array of drive.Entry.
//
//export ProtonDrive_ListDirectory
func ProtonDrive_ListDirectory(paramsJSON *C.char) *C.char {
	var p struct {
		UID    string `json:"uid"`
		LinkID string `json:"link_id"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	entries, err := sess.ListDirectory(context.Background(), p.LinkID)
	if err != nil {
		return errResult(err)
	}
	return okResult(entries)
}

// ProtonDrive_Stat returns metadata for a single link (file or
// folder). Input `{"uid":"...","link_id":"..."}`; result data is a
// `drive.Entry`, or null if the link is not in the active state.
//
//export ProtonDrive_Stat
func ProtonDrive_Stat(paramsJSON *C.char) *C.char {
	var p struct {
		UID    string `json:"uid"`
		LinkID string `json:"link_id"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	entry, err := sess.Stat(context.Background(), p.LinkID)
	if err != nil {
		return errResult(err)
	}
	return okResult(entry)
}

// ProtonDrive_DownloadFile downloads the active revision of a file
// link to a local path. Input `{"uid":"...","link_id":"...","dest_path":"..."}`.
// Blocks until the download is complete; result data is null on
// success.
//
//export ProtonDrive_DownloadFile
func ProtonDrive_DownloadFile(paramsJSON *C.char) *C.char {
	var p struct {
		UID      string `json:"uid"`
		LinkID   string `json:"link_id"`
		DestPath string `json:"dest_path"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	if err := sess.DownloadFile(context.Background(), p.LinkID, p.DestPath); err != nil {
		return errResult(err)
	}
	return okResult(nil)
}

// ProtonDrive_CreateFolder creates a folder named `name` under
// `parent_link_id` (empty parent = session root). Input
// `{"uid":"...","parent_link_id":"...","name":"..."}`; result data
// is the new folder's link ID as a string.
//
//export ProtonDrive_CreateFolder
func ProtonDrive_CreateFolder(paramsJSON *C.char) *C.char {
	var p struct {
		UID          string `json:"uid"`
		ParentLinkID string `json:"parent_link_id"`
		Name         string `json:"name"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	id, err := sess.CreateFolder(context.Background(), p.ParentLinkID, p.Name)
	if err != nil {
		return errResult(err)
	}
	return okResult(id)
}

// ProtonDrive_UploadFile uploads a local file as a new child of
// `parent_link_id` with the given `name`. Input JSON:
// `{"uid":"...","parent_link_id":"...","name":"...","src_path":"..."}`.
// Result data is the new file's link ID.
//
//export ProtonDrive_UploadFile
func ProtonDrive_UploadFile(paramsJSON *C.char) *C.char {
	var p struct {
		UID          string `json:"uid"`
		ParentLinkID string `json:"parent_link_id"`
		Name         string `json:"name"`
		SrcPath      string `json:"src_path"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	id, err := sess.UploadFile(context.Background(), p.ParentLinkID, p.Name, p.SrcPath)
	if err != nil {
		return errResult(err)
	}
	return okResult(id)
}

// ProtonDrive_TrashLink moves the named link into Proton's Trash.
// Works for both files and folders; for folders Proton's server
// cascade-trashes the contents. Input JSON:
// `{"uid":"...","link_id":"..."}`; result data is null on success.
//
//export ProtonDrive_TrashLink
func ProtonDrive_TrashLink(paramsJSON *C.char) *C.char {
	var p struct {
		UID    string `json:"uid"`
		LinkID string `json:"link_id"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	if err := sess.TrashLink(context.Background(), p.LinkID); err != nil {
		return errResult(err)
	}
	return okResult(nil)
}

// ProtonDrive_PermanentDeleteLink removes a link outright (no trash
// recovery window). Celeste's sync engine should NOT call this —
// mirror deletes route through TrashLink so the user has a window
// to restore. Exposed for completeness and for explicit
// "empty trash" UX later.
//
//export ProtonDrive_PermanentDeleteLink
func ProtonDrive_PermanentDeleteLink(paramsJSON *C.char) *C.char {
	var p struct {
		UID    string `json:"uid"`
		LinkID string `json:"link_id"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	if err := sess.PermanentDeleteLink(context.Background(), p.LinkID); err != nil {
		return errResult(err)
	}
	return okResult(nil)
}

// ProtonDrive_ListRecursive walks the tree rooted at `link_id` with
// concurrent API calls and returns a flat array of drive.Entry objects
// whose Name field is the full relative path. Pass an empty `link_id`
// to start from the session root.
//
//export ProtonDrive_ListRecursive
func ProtonDrive_ListRecursive(paramsJSON *C.char) *C.char {
	var p struct {
		UID    string `json:"uid"`
		LinkID string `json:"link_id"`
	}
	if err := json.Unmarshal([]byte(C.GoString(paramsJSON)), &p); err != nil {
		return errResult(err)
	}
	sess, err := drive.Lookup(p.UID)
	if err != nil {
		return errResult(err)
	}
	entries, err := sess.ListRecursive(context.Background(), p.LinkID)
	if err != nil {
		return errResult(err)
	}
	return okResult(entries)
}

// main is required by cgo for c-archive builds; body intentionally empty.
func main() {}
