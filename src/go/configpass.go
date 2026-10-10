package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"errors"

	"github.com/rclone/rclone/fs"
	"github.com/rclone/rclone/fs/config"
)

// rclone's own config encryption (NaCl secretbox), with a password Celeste keeps among its credentials: the rclone config file then holds the OAuth tokens only in encrypted form.

// CelesteSetConfigPassword makes rclone decrypt the config file with `password` and encrypt it on every save. Call it before anything reads the config. Returns NULL or an error message; free that with RcloneFreeString.
//
//export CelesteSetConfigPassword
func CelesteSetConfigPassword(password *C.char) *C.char {
	// A wrong password must not make rclone ask on the terminal.
	fs.GetConfig(context.Background()).AskPassword = false
	if err := config.SetConfigPassword(C.GoString(password)); err != nil {
		return C.CString(err.Error())
	}
	return nil
}

// CelesteSaveConfig loads the config file and writes it back, encrypted once a password is set. Leaves a file it cannot read untouched. Returns NULL or an error message; free that with RcloneFreeString.
//
//export CelesteSaveConfig
func CelesteSaveConfig() *C.char {
	data := config.Data()
	if err := data.Load(); err != nil && !errors.Is(err, config.ErrorConfigFileNotFound) {
		return C.CString(err.Error())
	}
	if err := data.Save(); err != nil {
		return C.CString(err.Error())
	}
	return nil
}
