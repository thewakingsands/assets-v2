package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"mime"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/minio/minio-go/v7"
	"github.com/minio/minio-go/v7/pkg/credentials"
)

var (
	iconRoutePattern = regexp.MustCompile(`^/i/\d{6}/(\d{6})(_hr1)?\.png$`)
	apiPathPattern   = regexp.MustCompile(`^ui/icon/\d{6}/(\d{6})(_hr1)?\.tex$`)
)

type iconRecord struct {
	ID      int    `json:"id"`
	Version string `json:"version"`
	HR      bool   `json:"hr"`
	SHA256  string `json:"sha256"`
	Format  string `json:"format"`
}

type currentReference struct {
	FFXIV          string `json:"ffxiv"`
	LastValidIndex string `json:"lastValidIndex"`
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
	store   *objectStore
	iconMap map[iconKey]assetRef
}

type objectStore struct {
	client   *minio.Client
	bucket   string
	prefix   string
	server   string
	cacheDir string
}

func main() {
	ctx := context.Background()

	store, err := newObjectStore()
	if err != nil {
		log.Fatal(err)
	}

	version := strings.TrimSpace(os.Getenv("ASSETS_VERSION"))
	if version == "" {
		version, err = store.loadCurrentVersion(ctx)
		if err != nil {
			log.Fatal(err)
		}
	}

	iconMap, err := loadIconMap(ctx, store, version)
	if err != nil {
		log.Fatal(err)
	}

	addr := envOrDefault("ASSETS_SERVER_ADDR", ":8080")
	srv := &server{
		store:   store,
		iconMap: iconMap,
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/i/", srv.handleIcon)
	mux.HandleFunc("/api/asset", srv.handleAPIAsset)
	handler := withCORS(mux)

	log.Printf("serving assets from minio bucket %s under %s", store.bucket, store.pathPrefix())
	if store.cacheDir != "" {
		log.Printf("serving cache from %s", store.cacheDir)
	}
	log.Printf("loaded %d icon mappings from %s", len(iconMap), version)
	log.Printf("listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, handler))
}

func withCORS(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Access-Control-Allow-Origin", "*")
		w.Header().Set("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
		w.Header().Set("Access-Control-Allow-Headers", "Content-Type")

		if r.Method == http.MethodOptions {
			w.WriteHeader(http.StatusNoContent)
			return
		}

		next.ServeHTTP(w, r)
	})
}

func newObjectStore() (*objectStore, error) {
	endpoint := requiredEnv("MINIO_ENDPOINT")
	accessKey := requiredEnv("MINIO_ACCESS_KEY")
	secretKey := requiredEnv("MINIO_SECRET_KEY")
	bucket := requiredEnv("MINIO_BUCKET_NAME")
	if endpoint == "" || accessKey == "" || secretKey == "" || bucket == "" {
		return nil, errors.New("MINIO_ENDPOINT, MINIO_ACCESS_KEY, MINIO_SECRET_KEY, and MINIO_BUCKET_NAME are required")
	}

	client, err := minio.New(endpoint, &minio.Options{
		Creds:  credentials.NewStaticV4(accessKey, secretKey, ""),
		Secure: envBool("MINIO_USE_SSL", true),
	})
	if err != nil {
		return nil, fmt.Errorf("create minio client: %w", err)
	}

	cacheDir, err := resolveCacheDir()
	if err != nil {
		return nil, err
	}

	return &objectStore{
		client:   client,
		bucket:   bucket,
		prefix:   trimStorageSegment(os.Getenv("MINIO_PREFIX")),
		server:   envOrDefault("ASSETS_SERVER", "sdo"),
		cacheDir: cacheDir,
	}, nil
}

func resolveCacheDir() (string, error) {
	if len(os.Args) > 1 && strings.TrimSpace(os.Args[1]) != "" {
		return filepath.Abs(os.Args[1])
	}

	for _, key := range []string{"ASSETS_CACHE_DIR", "ASSETS_DIR", "ASSETS_ROOT", "ASSETS_OUTPUT_DIR"} {
		if value := strings.TrimSpace(os.Getenv(key)); value != "" {
			return filepath.Abs(value)
		}
	}

	return "", nil
}

func (s *objectStore) loadCurrentVersion(ctx context.Context) (string, error) {
	data, _, err := s.readObject(ctx, "current.json")
	if err != nil {
		return "", fmt.Errorf("read current.json: %w", err)
	}

	var current currentReference
	if err := json.Unmarshal(data, &current); err != nil {
		return "", fmt.Errorf("parse current.json: %w", err)
	}

	if current.LastValidIndex != "" {
		return current.LastValidIndex, nil
	}
	if current.FFXIV != "" {
		return current.FFXIV, nil
	}

	return "", errors.New("current.json does not contain a version")
}

func loadIconMap(ctx context.Context, store *objectStore, version string) (map[iconKey]assetRef, error) {
	data, err := store.readIndex(ctx, version)
	if err != nil {
		return nil, err
	}

	var records []iconRecord
	if err := json.Unmarshal(data, &records); err != nil {
		return nil, fmt.Errorf("parse icons index: %w", err)
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

func (s *objectStore) readIndex(ctx context.Context, version string) ([]byte, error) {
	cachePath := filepath.Join(s.cacheDir, filepath.FromSlash(indexPath(version)))
	if s.cacheDir != "" {
		if data, err := os.ReadFile(cachePath); err == nil {
			return data, nil
		}
	}

	relativePath := fmt.Sprintf("patches/%s/icons.json", version)
	data, _, err := s.readObject(ctx, relativePath)
	if err != nil {
		return nil, fmt.Errorf("read %s: %w", relativePath, err)
	}

	if s.cacheDir != "" {
		if err := writeFile(cachePath, data); err != nil {
			log.Printf("cache index write failed: %v", err)
		}
	}

	return data, nil
}

func (s *server) serveAsset(w http.ResponseWriter, r *http.Request, ref assetRef) {
	relativePath := assetPath(ref.SHA256, ref.Format)

	if s.store.cacheDir != "" {
		path := filepath.Join(s.store.cacheDir, filepath.FromSlash(relativePath))
		if _, err := os.Stat(path); err == nil {
			http.ServeFile(w, r, path)
			return
		} else if !errors.Is(err, os.ErrNotExist) {
			http.Error(w, "stat cached asset failed", http.StatusInternalServerError)
			return
		}
	}

	data, contentType, err := s.store.readObject(r.Context(), relativePath)
	if err != nil {
		if isMinioNotFound(err) {
			http.NotFound(w, r)
			return
		}

		http.Error(w, "read asset failed", http.StatusInternalServerError)
		return
	}

	if s.store.cacheDir != "" {
		path := filepath.Join(s.store.cacheDir, filepath.FromSlash(relativePath))
		if err := writeFile(path, data); err != nil {
			log.Printf("cache asset write failed: %v", err)
		} else {
			http.ServeFile(w, r, path)
			return
		}
	}

	if contentType == "" {
		contentType = contentTypeForExt(ref.Format)
	}
	if contentType != "" {
		w.Header().Set("Content-Type", contentType)
	}
	http.ServeContent(w, r, ref.SHA256+"."+ref.Format, time.Now(), bytes.NewReader(data))
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

	s.serveAsset(w, r, ref)
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

	s.serveAsset(w, r, ref)
}

func (s *objectStore) readObject(ctx context.Context, relativePath string) ([]byte, string, error) {
	object, err := s.client.GetObject(ctx, s.bucket, s.objectName(relativePath), minio.GetObjectOptions{})
	if err != nil {
		return nil, "", err
	}
	defer object.Close()

	info, err := object.Stat()
	if err != nil {
		return nil, "", err
	}

	data, err := io.ReadAll(object)
	if err != nil {
		return nil, "", err
	}

	return data, info.ContentType, nil
}

func (s *objectStore) objectName(relativePath string) string {
	return joinStorageSegments(s.pathPrefix(), relativePath)
}

func (s *objectStore) pathPrefix() string {
	return joinStorageSegments(s.prefix, "ui", s.server)
}

func assetPath(sha256 string, ext string) string {
	return fmt.Sprintf("assets/%s/%s.%s", sha256[:2], sha256, ext)
}

func indexPath(version string) string {
	return fmt.Sprintf("indexes/%s.json", version)
}

func writeFile(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}

	return os.WriteFile(path, data, 0o644)
}

func isMinioNotFound(err error) bool {
	response := minio.ToErrorResponse(err)
	return response.StatusCode == http.StatusNotFound || response.Code == "NoSuchKey"
}

func contentTypeForExt(ext string) string {
	switch ext {
	case "avif":
		return "image/avif"
	case "webp":
		return "image/webp"
	default:
		return mime.TypeByExtension("." + ext)
	}
}

func requiredEnv(key string) string {
	return strings.TrimSpace(os.Getenv(key))
}

func envOrDefault(key, fallback string) string {
	if value := strings.TrimSpace(os.Getenv(key)); value != "" {
		return value
	}

	return fallback
}

func envBool(key string, fallback bool) bool {
	value := strings.ToLower(strings.TrimSpace(os.Getenv(key)))
	if value == "" {
		return fallback
	}

	return value == "1" || value == "true" || value == "yes" || value == "on"
}

func trimStorageSegment(value string) string {
	return strings.Trim(strings.TrimSpace(value), "/")
}

func joinStorageSegments(parts ...string) string {
	segments := make([]string, 0, len(parts))
	for _, part := range parts {
		trimmed := trimStorageSegment(part)
		if trimmed != "" {
			segments = append(segments, trimmed)
		}
	}

	return strings.Join(segments, "/")
}
