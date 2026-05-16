package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

var (
	iconRoutePattern = regexp.MustCompile(`^/i/\d{6}/(\d{6})(_hr1)?\.png$`)
	apiPathPattern   = regexp.MustCompile(`^ui/icon/\d{6}/(\d{6})(_hr1)?\.tex$`)
	assetPattern     = regexp.MustCompile(`^/assets/([0-9a-f]{64})\.([a-z0-9]+)$`)
)

type iconRecord struct {
	ID      int    `json:"id"`
	Version string `json:"version"`
	HR      bool   `json:"hr"`
	SHA256  string `json:"sha256"`
	Format  string `json:"format"`
}

type assetRef struct {
	SHA256 string
	Format string
}

type iconKey struct {
	ID      int
	Version string
	HR      bool
}

type server struct {
	root    string
	iconMap map[iconKey]assetRef
}

func main() {
	root, err := resolveRoot()
	if err != nil {
		log.Fatal(err)
	}

	iconMap, err := loadIconMap(root)
	if err != nil {
		log.Fatal(err)
	}

	addr := envOrDefault("ASSETS_SERVER_ADDR", ":8080")
	srv := &server{
		root:    root,
		iconMap: iconMap,
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/assets/", srv.handleAsset)
	mux.HandleFunc("/i/", srv.handleIcon)
	mux.HandleFunc("/api/asset", srv.handleAPIAsset)

	log.Printf("serving assets from %s", root)
	log.Printf("loaded %d icon mappings", len(iconMap))
	log.Printf("listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, mux))
}

func resolveRoot() (string, error) {
	if len(os.Args) > 1 && strings.TrimSpace(os.Args[1]) != "" {
		return filepath.Abs(os.Args[1])
	}

	for _, key := range []string{"ASSETS_DIR", "ASSETS_ROOT", "ASSETS_OUTPUT_DIR"} {
		if value := strings.TrimSpace(os.Getenv(key)); value != "" {
			return filepath.Abs(value)
		}
	}

	return filepath.Abs("outputs")
}

func loadIconMap(root string) (map[iconKey]assetRef, error) {
	iconsPath := filepath.Join(root, "icons.json")
	data, err := os.ReadFile(iconsPath)
	if err != nil {
		return nil, fmt.Errorf("read icons.json: %w", err)
	}

	var records []iconRecord
	if err := json.Unmarshal(data, &records); err != nil {
		return nil, fmt.Errorf("parse icons.json: %w", err)
	}

	iconMap := make(map[iconKey]assetRef, len(records))
	for _, record := range records {
		if record.SHA256 == "" || record.Format == "" {
			continue
		}

		iconMap[iconKey{
			ID:      record.ID,
			Version: record.Version,
			HR:      record.HR,
		}] = assetRef{
			SHA256: record.SHA256,
			Format: record.Format,
		}
	}

	return iconMap, nil
}

func (s *server) handleAsset(w http.ResponseWriter, r *http.Request) {
	matches := assetPattern.FindStringSubmatch(r.URL.Path)
	if matches == nil {
		http.NotFound(w, r)
		return
	}

	sha256 := matches[1]
	ext := matches[2]
	path := filepath.Join(s.root, sha256[:2], sha256+"."+ext)
	if _, err := os.Stat(path); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			http.NotFound(w, r)
			return
		}

		http.Error(w, "stat asset failed", http.StatusInternalServerError)
		return
	}

	http.ServeFile(w, r, path)
}

func (s *server) handleIcon(w http.ResponseWriter, r *http.Request) {
	matches := iconRoutePattern.FindStringSubmatch(r.URL.Path)
	if matches == nil {
		http.NotFound(w, r)
		return
	}

	id, err := strconv.Atoi(matches[1])
	if err != nil {
		http.NotFound(w, r)
		return
	}

	ref, ok := s.iconMap[iconKey{ID: id, Version: "", HR: matches[2] != ""}]
	if !ok {
		http.NotFound(w, r)
		return
	}

	http.Redirect(w, r, assetURL(ref), http.StatusFound)
}

func (s *server) handleAPIAsset(w http.ResponseWriter, r *http.Request) {
	rawPath := strings.TrimSpace(r.URL.Query().Get("path"))
	if rawPath == "" {
		http.Error(w, "missing path query", http.StatusBadRequest)
		return
	}

	path, err := url.QueryUnescape(rawPath)
	if err == nil {
		rawPath = path
	}

	matches := apiPathPattern.FindStringSubmatch(rawPath)
	if matches == nil {
		http.NotFound(w, r)
		return
	}

	id, err := strconv.Atoi(matches[1])
	if err != nil {
		http.NotFound(w, r)
		return
	}

	ref, ok := s.iconMap[iconKey{ID: id, Version: "", HR: matches[2] != ""}]
	if !ok {
		http.NotFound(w, r)
		return
	}

	http.Redirect(w, r, assetURL(ref), http.StatusFound)
}

func assetURL(ref assetRef) string {
	return fmt.Sprintf("/assets/%s.%s", ref.SHA256, ref.Format)
}

func envOrDefault(key, fallback string) string {
	if value := strings.TrimSpace(os.Getenv(key)); value != "" {
		return value
	}

	return fallback
}
