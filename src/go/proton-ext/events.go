package protonext

// Drive volume events. Upstream has GetVolumeEvent, but its paging loop re-requests the first page's EventID on every iteration and spins forever once more than two pages are pending, so Celeste pages through the endpoint itself.

import (
	"context"
	"encoding/json"

	"github.com/ProtonMail/go-proton-api"
)

// VolumeEventPage is one page of the volume's event log. The individual events are left undecoded: Celeste only needs to know whether anything changed, not what.
type VolumeEventPage struct {
	EventID string
	Events  []json.RawMessage
	Refresh proton.Bool
	More    proton.Bool
}

// LatestVolumeEventID returns the cursor of the newest event on `volumeID`; events after it are fetched with VolumeEvents.
func LatestVolumeEventID(ctx context.Context, auth Auth, volumeID string) (string, error) {
	var res struct {
		EventID string
	}
	if err := do(ctx, auth, "GET", "/drive/volumes/"+volumeID+"/events/latest", nil, &res); err != nil {
		return "", err
	}
	return res.EventID, nil
}

// VolumeEvents returns the page of events following `eventID`. The page's EventID is the cursor for the next call; More is set when further pages are already waiting.
func VolumeEvents(ctx context.Context, auth Auth, volumeID, eventID string) (VolumeEventPage, error) {
	var page VolumeEventPage
	if err := do(ctx, auth, "GET", "/drive/volumes/"+volumeID+"/events/"+eventID, nil, &page); err != nil {
		return VolumeEventPage{}, err
	}
	return page, nil
}
