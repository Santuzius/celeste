package main

import (
	"context"
	"fmt"
	"os"
	"sort"
	"strings"
	"time"

	"celeste/go/drive"

	"github.com/ProtonMail/go-proton-api"
)

func main() {
	home, err := os.UserHomeDir()
	if err != nil {
		fmt.Fprintf(os.Stderr, "cannot find home dir: %v\n", err)
		os.Exit(1)
	}
	// Celeste itself stores the session blob in the OS keyring under
	// `Celeste Keys / proton-session-<remote>`; this tool still wants
	// a JSON path. Pass one explicitly (e.g. exported with
	// `secret-tool lookup` first), or fall back to the legacy file
	// location for users who haven't yet upgraded past the keyring
	// migration.
	sessionPath := home + "/.local/share/celeste/proton-session-ProtonDrive.json"
	if len(os.Args) > 1 {
		sessionPath = os.Args[1]
	}

	targetName := "Text File.txt"
	if len(os.Args) > 2 {
		targetName = os.Args[2]
	}

	mode := "diagnose"
	if len(os.Args) > 3 {
		mode = os.Args[3]
	}

	fmt.Printf("=== Celeste ProtonDrive Diagnostic Tool ===\n")
	fmt.Printf("Session file: %s\n", sessionPath)
	fmt.Printf("Target:       %q\n", targetName)
	fmt.Printf("Mode:         %s\n\n", mode)

	cred, err := drive.LoadCredential(sessionPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "LoadCredential: %v\n", err)
		os.Exit(1)
	}

	ctx := context.Background()
	sess, err := drive.Resume(ctx, cred)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Resume: %v\n", err)
		os.Exit(1)
	}
	defer sess.Close()

	rootID := sess.RootLinkID()
	fmt.Printf("Root link ID: %s\n\n", rootID)

	switch mode {
	case "diagnose":
		searchFolder(ctx, sess, rootID, "/", targetName)
	case "fix":
		fixGhost(ctx, sess, rootID, targetName)
	case "upload-test":
		fixGhost(ctx, sess, rootID, targetName)
		uploadTest(ctx, sess, rootID, targetName)
	case "bench-list":
		benchList(ctx, sess, rootID)
	case "bench-recursive":
		benchRecursive(ctx, sess, rootID)
	case "audit":
		auditStorage(ctx, sess, false)
	case "audit-revisions":
		auditStorage(ctx, sess, true)
	case "purge-trash":
		purgeNonActive(ctx, sess, proton.LinkStateTrashed)
	case "purge-drafts":
		purgeNonActive(ctx, sess, proton.LinkStateDraft)
	case "purge-obsolete-revisions":
		purgeObsoleteRevisions(ctx, sess)
	case "replace-test":
		replaceTest(ctx, sess, rootID, targetName)
	default:
		fmt.Fprintf(os.Stderr, "unknown mode %q (use: diagnose, fix, upload-test, replace-test, bench-list, bench-recursive, audit, audit-revisions, purge-trash, purge-drafts, purge-obsolete-revisions)\n", mode)
		os.Exit(1)
	}

	fmt.Printf("\n=== Done ===\n")
}

func searchFolder(ctx context.Context, sess *drive.Session, folderLinkID, path, targetName string) {
	allEntries, err := sess.ListAllChildren(ctx, folderLinkID)
	if err != nil {
		fmt.Printf("  ERROR listing %s (%s): %v\n", path, folderLinkID, err)
		return
	}

	total := len(allEntries)
	matches := 0
	stateCount := map[int]int{}

	for _, e := range allEntries {
		stateCount[e.State]++

		if strings.EqualFold(e.Name, targetName) || e.Name == targetName {
			matches++
			stateName := linkStateName(e.State)
			fmt.Printf(">>> MATCH FOUND in %s:\n", path)
			fmt.Printf("    LinkID:       %s\n", e.LinkID)
			fmt.Printf("    Name:         %q\n", e.Name)
			fmt.Printf("    IsDir:        %v\n", e.IsDir)
			fmt.Printf("    State:        %d (%s)\n", e.State, stateName)
			fmt.Printf("    Size:         %d\n", e.Size)
			fmt.Printf("    MIMEType:     %s\n", e.MIMEType)
			fmt.Println()
		}
	}

	fmt.Printf("Summary: %d total children, %d matches\n", total, matches)
	fmt.Printf("  By state: Active=%d Draft=%d Trashed=%d Deleted=%d Other=%d\n",
		stateCount[int(proton.LinkStateActive)],
		stateCount[int(proton.LinkStateDraft)],
		stateCount[int(proton.LinkStateTrashed)],
		stateCount[int(proton.LinkStateDeleted)],
		total-stateCount[int(proton.LinkStateActive)]-stateCount[int(proton.LinkStateDraft)]-stateCount[int(proton.LinkStateTrashed)]-stateCount[int(proton.LinkStateDeleted)])
}

func fixGhost(ctx context.Context, sess *drive.Session, rootID, targetName string) {
	fmt.Printf("--- Fix: searching for ghost %q ---\n", targetName)

	allEntries, err := sess.ListAllChildren(ctx, rootID)
	if err != nil {
		fmt.Printf("  ERROR listing root: %v\n", err)
		return
	}

	found := false
	for _, e := range allEntries {
		if e.Name != targetName || e.IsDir {
			continue
		}
		if e.State == int(proton.LinkStateActive) {
			fmt.Printf("  File %q is Active (linkID=%s) — not a ghost, skipping.\n", e.Name, e.LinkID)
			continue
		}

		found = true
		stateName := linkStateName(e.State)
		fmt.Printf("  Found ghost: linkID=%s state=%d (%s)\n", e.LinkID, e.State, stateName)
		fmt.Printf("  Attempting to permanently delete via DeleteChildren...\n")

		// Use UploadFile which internally calls createFileDraft which
		// handles the conflict. But for a targeted fix we can use the
		// session's exported method. Since we can't call DeleteChildren
		// directly from here (it's on the proton client, not Session),
		// we use a small test upload to trigger the conflict handler.
		// Instead, let's just report and let the upload-test mode handle it.
		fmt.Printf("  Ghost found. Use 'upload-test' mode to trigger the conflict handler and verify upload works.\n")
	}

	if !found {
		fmt.Printf("  No ghost found for %q — the name is clear.\n", targetName)
	}
}

// replaceTest uploads `targetName` twice with *different* content. The
// second upload should hit the conflict path, create a new revision,
// and (post-fix) trigger the obsolete-revision cleanup so the file
// ends up with exactly one revision when the dust settles.
func replaceTest(ctx context.Context, sess *drive.Session, rootID, targetName string) {
	fmt.Printf("\n--- Replace Test: upload %q twice with different content ---\n", targetName)

	upload := func(label, content string) string {
		tmpFile, err := os.CreateTemp("", "celeste-replace-*.txt")
		if err != nil {
			fmt.Printf("  ERROR creating temp file: %v\n", err)
			return ""
		}
		tmpPath := tmpFile.Name()
		defer os.Remove(tmpPath)
		if _, err := tmpFile.WriteString(content); err != nil {
			tmpFile.Close()
			fmt.Printf("  ERROR writing temp file: %v\n", err)
			return ""
		}
		tmpFile.Close()

		fmt.Printf("  [%s] uploading %d bytes …\n", label, len(content))
		linkID, err := sess.UploadFile(ctx, rootID, targetName, tmpPath)
		if err != nil {
			fmt.Printf("  [%s] UPLOAD FAILED: %v\n", label, err)
			return ""
		}
		fmt.Printf("  [%s] OK linkID=%s\n", label, linkID)
		return linkID
	}

	stamp := time.Now().UnixNano()
	first := upload("first", fmt.Sprintf("celeste replace-test v1 — %d\n", stamp))
	second := upload("second", fmt.Sprintf("celeste replace-test v2 — %d (longer body so SHA1 differs)\n", stamp))

	if first == "" || second == "" {
		fmt.Printf("  Upload(s) failed; cannot inspect revisions.\n")
		return
	}
	if first != second {
		fmt.Printf("  WARNING: second upload returned a different linkID (%s != %s) — expected reuse.\n", second, first)
	}

	// Pause so the API has a moment to settle, then audit the file's
	// revision count via a tree-walk (cheap; we only care about this
	// one file).
	time.Sleep(1 * time.Second)
	report, err := sess.AuditStorage(ctx, drive.AuditOptions{IncludeRevisions: true})
	if err != nil {
		fmt.Printf("  AuditStorage failed: %v\n", err)
		return
	}
	count := 0
	for _, r := range report.ObsoleteRevisions {
		if r.FileLinkID == second {
			count++
		}
	}
	fmt.Printf("  Obsolete revisions on %s after second upload: %d\n", second, count)
	if count == 0 {
		fmt.Printf("  ✓ Cleanup ran — no obsolete revisions remain.\n")
	} else {
		fmt.Printf("  ✗ Cleanup did NOT run — %d obsolete revisions remain on this file.\n", count)
	}
}

func uploadTest(ctx context.Context, sess *drive.Session, rootID, targetName string) {
	fmt.Printf("\n--- Upload Test: uploading %q to root ---\n", targetName)

	// Create a small temp file to upload
	tmpFile, err := os.CreateTemp("", "celeste-diag-*.txt")
	if err != nil {
		fmt.Printf("  ERROR creating temp file: %v\n", err)
		return
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	_, err = tmpFile.WriteString("Celeste diagnostic upload test\n")
	if err != nil {
		tmpFile.Close()
		fmt.Printf("  ERROR writing temp file: %v\n", err)
		return
	}
	tmpFile.Close()

	fmt.Printf("  Temp file: %s\n", tmpPath)
	fmt.Printf("  Calling UploadFile(root, %q, %q)...\n", targetName, tmpPath)

	linkID, err := sess.UploadFile(ctx, rootID, targetName, tmpPath)
	if err != nil {
		fmt.Printf("  UPLOAD FAILED: %v\n", err)
		return
	}

	fmt.Printf("  UPLOAD SUCCEEDED! New file linkID: %s\n", linkID)

	// Verify it's visible now
	fmt.Printf("\n  Verifying file is now visible in listing...\n")
	searchFolder(ctx, sess, rootID, "/", targetName)
}

func benchList(ctx context.Context, sess *drive.Session, rootID string) {
	fmt.Printf("--- Benchmark: full recursive listing ---\n\n")

	type folderInfo struct {
		linkID string
		path   string
	}

	start := time.Now()
	var totalFiles, totalDirs, totalAPIcalls int

	stack := []folderInfo{{linkID: rootID, path: "/"}}
	for len(stack) > 0 {
		cur := stack[len(stack)-1]
		stack = stack[:len(stack)-1]

		t0 := time.Now()
		entries, err := sess.ListDirectory(ctx, cur.linkID)
		elapsed := time.Since(t0)
		totalAPIcalls++

		if err != nil {
			fmt.Printf("  ERROR listing %s: %v\n", cur.path, err)
			continue
		}

		nFiles, nDirs := 0, 0
		for _, e := range entries {
			if e.IsDir {
				nDirs++
				childPath := cur.path
				if childPath == "/" {
					childPath = "/" + e.Name
				} else {
					childPath = cur.path + "/" + e.Name
				}
				stack = append(stack, folderInfo{linkID: e.LinkID, path: childPath})
			} else {
				nFiles++
			}
		}
		totalFiles += nFiles
		totalDirs += nDirs

		if elapsed > 500*time.Millisecond {
			fmt.Printf("  SLOW %6dms  %s (%d files, %d dirs)\n", elapsed.Milliseconds(), cur.path, nFiles, nDirs)
		}
	}

	totalElapsed := time.Since(start)
	fmt.Printf("\nResults:\n")
	fmt.Printf("  Total time:      %v\n", totalElapsed.Round(time.Millisecond))
	fmt.Printf("  Total files:     %d\n", totalFiles)
	fmt.Printf("  Total dirs:      %d\n", totalDirs)
	fmt.Printf("  API calls:       %d\n", totalAPIcalls)
	fmt.Printf("  Avg per call:    %v\n", (totalElapsed / time.Duration(totalAPIcalls)).Round(time.Millisecond))
}

func benchRecursive(ctx context.Context, sess *drive.Session, rootID string) {
	fmt.Printf("--- Benchmark: concurrent recursive listing (ListRecursive) ---\n\n")

	start := time.Now()
	entries, err := sess.ListRecursive(ctx, rootID)
	elapsed := time.Since(start)

	if err != nil {
		fmt.Printf("  ERROR: %v\n", err)
		return
	}

	nFiles, nDirs := 0, 0
	for _, e := range entries {
		if e.IsDir {
			nDirs++
		} else {
			nFiles++
		}
	}

	fmt.Printf("Results:\n")
	fmt.Printf("  Total time:      %v\n", elapsed.Round(time.Millisecond))
	fmt.Printf("  Total files:     %d\n", nFiles)
	fmt.Printf("  Total dirs:      %d\n", nDirs)
	fmt.Printf("  Total entries:   %d\n", len(entries))
}

func linkStateName(state int) string {
	switch proton.LinkState(state) {
	case proton.LinkStateDraft:
		return "Draft"
	case proton.LinkStateActive:
		return "Active"
	case proton.LinkStateTrashed:
		return "Trashed"
	case proton.LinkStateDeleted:
		return "Deleted"
	case proton.LinkStateRestoring:
		return "Restoring"
	default:
		return fmt.Sprintf("Unknown(%d)", state)
	}
}

func revisionStateName(s proton.RevisionState) string {
	switch s {
	case proton.RevisionStateDraft:
		return "Draft"
	case proton.RevisionStateActive:
		return "Active"
	case proton.RevisionStateObsolete:
		return "Obsolete"
	case proton.RevisionStateDeleted:
		return "Deleted"
	default:
		return fmt.Sprintf("Unknown(%d)", int(s))
	}
}

func humanSize(bytes int64) string {
	const unit = 1024
	if bytes < unit {
		return fmt.Sprintf("%d B", bytes)
	}
	div, exp := int64(unit), 0
	for n := bytes / unit; n >= unit; n /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.2f %ciB", float64(bytes)/float64(div), "KMGTPE"[exp])
}

func humanSizeU(bytes uint64) string {
	return humanSize(int64(bytes))
}

func auditStorage(ctx context.Context, sess *drive.Session, includeRevisions bool) {
	fmt.Printf("--- Storage Audit ---\n")
	if includeRevisions {
		fmt.Printf("(walking the full tree + listing revisions for every active file — slow)\n\n")
	} else {
		fmt.Printf("(walking the full tree — including trashed/draft folders)\n\n")
	}

	start := time.Now()
	report, err := sess.AuditStorage(ctx, drive.AuditOptions{
		IncludeRevisions: includeRevisions,
	})
	if err != nil {
		fmt.Printf("  ERROR: %v\n", err)
		return
	}
	elapsed := time.Since(start)

	fmt.Printf("Account-level storage:\n")
	fmt.Printf("  User UsedSpace:      %s\n", humanSizeU(report.UserUsedSpace))
	fmt.Printf("  User MaxSpace:       %s\n", humanSizeU(report.UserMaxSpace))
	fmt.Printf("  Drive UsedSpace:     %s\n", humanSizeU(report.DriveUsedSpace))
	fmt.Printf("\n")
	fmt.Printf("Volume:\n")
	fmt.Printf("  UsedSpace (volume):  %s\n", humanSize(report.VolumeUsedSpace))
	if report.VolumeMaxSpace != nil {
		fmt.Printf("  MaxSpace (volume):   %s\n", humanSize(*report.VolumeMaxSpace))
	} else {
		fmt.Printf("  MaxSpace (volume):   unlimited\n")
	}
	fmt.Printf("\n")

	fmt.Printf("Tree walk: %d total links\n", report.TotalLinksWalked)
	states := []proton.LinkState{
		proton.LinkStateActive,
		proton.LinkStateDraft,
		proton.LinkStateTrashed,
		proton.LinkStateDeleted,
		proton.LinkStateRestoring,
	}
	for _, st := range states {
		count := report.StateCounts[st]
		size := report.StateSizes[st]
		if count == 0 && size == 0 {
			continue
		}
		fmt.Printf("  %-9s  count=%-5d  size=%s\n", linkStateName(int(st)), count, humanSize(size))
	}
	fmt.Printf("\n")

	dumpEntries := func(label string, entries []drive.AuditEntry) {
		if len(entries) == 0 {
			return
		}
		// Sort largest first so the worst offenders surface.
		sort.Slice(entries, func(i, j int) bool { return entries[i].Size > entries[j].Size })
		var total int64
		for _, e := range entries {
			total += e.Size
		}
		fmt.Printf("%s entries: %d (%s total)\n", label, len(entries), humanSize(total))
		const max = 30
		shown := len(entries)
		if shown > max {
			shown = max
		}
		for i := 0; i < shown; i++ {
			e := entries[i]
			kind := "file"
			if e.IsDir {
				kind = "dir "
			}
			fmt.Printf("  %s  %10s  %s  (linkID=%s)\n", kind, humanSize(e.Size), e.Path, e.LinkID)
		}
		if len(entries) > max {
			fmt.Printf("  … %d more not shown\n", len(entries)-max)
		}
		fmt.Printf("\n")
	}

	dumpEntries("Trashed", report.Trashed)
	dumpEntries("Draft", report.Drafts)
	dumpEntries("Deleted", report.Deleted)

	if includeRevisions {
		var totalRevSize int64
		for _, r := range report.ObsoleteRevisions {
			totalRevSize += r.Size
		}
		fmt.Printf("Obsolete/Draft revisions on active files: %d (%s total)\n",
			len(report.ObsoleteRevisions), humanSize(totalRevSize))
		// Sort by size desc.
		revs := report.ObsoleteRevisions
		sort.Slice(revs, func(i, j int) bool { return revs[i].Size > revs[j].Size })
		const max = 30
		shown := len(revs)
		if shown > max {
			shown = max
		}
		for i := 0; i < shown; i++ {
			r := revs[i]
			ts := time.Unix(r.CreateTime, 0).Format("2006-01-02 15:04:05")
			fmt.Printf("  %-8s  %10s  %s  (file=%s rev=%s created=%s)\n",
				revisionStateName(r.State), humanSize(r.Size), r.FilePath, r.FileLinkID, r.RevisionID, ts)
		}
		if len(revs) > max {
			fmt.Printf("  … %d more not shown\n", len(revs)-max)
		}
		fmt.Printf("\n")
	}

	// Sanity check: how much space should be reclaimable if we purge
	// trashed + drafts + deleted + obsolete revisions?
	var reclaimable int64
	reclaimable += report.StateSizes[proton.LinkStateTrashed]
	reclaimable += report.StateSizes[proton.LinkStateDraft]
	reclaimable += report.StateSizes[proton.LinkStateDeleted]
	for _, r := range report.ObsoleteRevisions {
		reclaimable += r.Size
	}
	fmt.Printf("Estimated reclaimable: %s (purging trash + drafts + deleted%s)\n",
		humanSize(reclaimable),
		map[bool]string{true: " + obsolete revisions", false: ""}[includeRevisions])
	fmt.Printf("Audit walk time: %v\n", elapsed.Round(time.Millisecond))
}

// purgeNonActive permanently deletes every link in the given non-active
// state (Trashed or Draft). Groups link IDs by their parent so each
// DeleteChildren call takes exactly the children that belong to one
// folder, which is what Proton's API requires.
func purgeNonActive(ctx context.Context, sess *drive.Session, target proton.LinkState) {
	if target != proton.LinkStateTrashed && target != proton.LinkStateDraft {
		fmt.Printf("  ERROR: only Trashed/Draft are supported for purge.\n")
		return
	}

	fmt.Printf("--- Purge %s ---\n", linkStateName(int(target)))
	report, err := sess.AuditStorage(ctx, drive.AuditOptions{})
	if err != nil {
		fmt.Printf("  ERROR auditing first: %v\n", err)
		return
	}

	var pool []drive.AuditEntry
	switch target {
	case proton.LinkStateTrashed:
		pool = report.Trashed
	case proton.LinkStateDraft:
		pool = report.Drafts
	}

	// When trashing a folder, the server cascade-trashes its contents.
	// In the audit walk we descend into trashed folders too, so a
	// single trashed parent will pull a whole subtree of trashed
	// children into `pool`. Permanently deleting the parent purges the
	// children as well; calling DeleteChildren on those children
	// individually after their parent is gone fails. So: keep only the
	// shallowest trashed link in each subtree.
	idSet := make(map[string]bool, len(pool))
	for _, e := range pool {
		idSet[e.LinkID] = true
	}
	roots := pool[:0]
	for _, e := range pool {
		if idSet[e.ParentLinkID] {
			continue // a shallower entry in this subtree already covers it
		}
		roots = append(roots, e)
	}

	if len(roots) == 0 {
		fmt.Printf("  Nothing to purge — no top-level %s entries found.\n", linkStateName(int(target)))
		return
	}

	byParent := map[string][]string{}
	var totalSize int64
	for _, e := range roots {
		byParent[e.ParentLinkID] = append(byParent[e.ParentLinkID], e.LinkID)
		totalSize += e.Size
	}
	fmt.Printf("  Will permanently delete %d top-level %s entries (%s, across %d parent folders).\n",
		len(roots), linkStateName(int(target)), humanSize(totalSize), len(byParent))
	fmt.Printf("  This is IRREVERSIBLE.\n\n")

	for parentID, ids := range byParent {
		fmt.Printf("  Purging %d children under parent %s …\n", len(ids), parentID)
		if err := sess.PurgeLinks(ctx, parentID, ids); err != nil {
			fmt.Printf("    ERROR: %v\n", err)
			continue
		}
		fmt.Printf("    OK\n")
	}
}

// purgeObsoleteRevisions deletes every revision on every active file
// that is not the current Active revision. Walks the tree first to find
// every file, then issues DeleteRevision per non-active revision.
func purgeObsoleteRevisions(ctx context.Context, sess *drive.Session) {
	fmt.Printf("--- Purge Obsolete/Draft Revisions ---\n")
	report, err := sess.AuditStorage(ctx, drive.AuditOptions{IncludeRevisions: true})
	if err != nil {
		fmt.Printf("  ERROR auditing first: %v\n", err)
		return
	}
	if len(report.ObsoleteRevisions) == 0 {
		fmt.Printf("  No obsolete revisions found.\n")
		return
	}
	var totalSize int64
	for _, r := range report.ObsoleteRevisions {
		totalSize += r.Size
	}
	fmt.Printf("  Will permanently delete %d non-active revisions (%s).\n",
		len(report.ObsoleteRevisions), humanSize(totalSize))
	fmt.Printf("  This is IRREVERSIBLE.\n\n")

	for _, r := range report.ObsoleteRevisions {
		fmt.Printf("  Deleting %s revision %s on %s (%s) …\n",
			revisionStateName(r.State), r.RevisionID, r.FilePath, humanSize(r.Size))
		if err := sess.PurgeRevision(ctx, r.FileLinkID, r.RevisionID); err != nil {
			fmt.Printf("    ERROR: %v\n", err)
			continue
		}
		fmt.Printf("    OK\n")
	}
}
