import type { FileContents } from "../src";

// An invented Go test file before and after a change, with strings, keywords, function names,
// numbers, and comments, so every Sigil code color shows.
export const oldFile: FileContents = {
  name: "review/score_test.go",
  contents: `package review

import (
	"testing"
	"time"
)

// TestScoreThreshold checks that scores at the boundary pass.
func TestScoreThreshold(t *testing.T) {
	cases := []struct {
		name  string
		score float64
		want  bool
	}{
		{"below", 0.49, false},
		{"exact", 0.5, true},
		{"above", 0.75, true},
	}
	for _, tc := range cases {
		got := passes(tc.score, 0.5)
		if got != tc.want {
			t.Errorf("passes(%v) = %v, want %v", tc.score, got, tc.want)
		}
	}
}

func TestTimeout(t *testing.T) {
	start := time.Now()
	waitFor(100 * time.Millisecond)
	if time.Since(start) < 100*time.Millisecond {
		t.Fatal("returned too early")
	}
}
`,
};

export const newFile: FileContents = {
  name: "review/score_test.go",
  contents: `package review

import (
	"testing"
	"time"
)

// passThreshold is the lowest score that counts as a pass.
const passThreshold = 0.5

// TestScoreThreshold checks that scores at the boundary pass.
func TestScoreThreshold(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name  string
		score float64
		want  bool
	}{
		{"below", 0.49, false},
		{"exact", passThreshold, true},
		{"above", 0.75, true},
		{"max", 1.0, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := passes(tc.score, passThreshold)
			if got != tc.want {
				t.Errorf("passes(%v) = %v, want %v", tc.score, got, tc.want)
			}
		})
	}
}

func TestTimeout(t *testing.T) {
	start := time.Now()
	waitFor(250 * time.Millisecond)
	if elapsed := time.Since(start); elapsed < 250*time.Millisecond {
		t.Fatalf("returned after %v, want at least 250ms", elapsed)
	}
}
`,
};

export interface DraftComment {
  id: string;
  line: number;
  label: string;
  body: string;
}

export const drafts: DraftComment[] = [
  {
    id: "c1",
    line: 9,
    label: "nit",
    body: "Name the constant after what it gates, for example minPassingScore, so the cases read as rules.",
  },
  {
    id: "c2",
    line: 25,
    label: "question",
    body: "These subtests don't call t.Parallel(), so only the parent test runs in parallel. Is that intended?",
  },
];
