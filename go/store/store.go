// Package store writes points to InfluxDB v2 via influxdb-client-go/v2.
// There is deliberately no ensure-database step: buckets are
// operator-provisioned, so startup is ping/ready, then go.
package store

import (
	"context"
	"fmt"
	"log/slog"
	"time"

	influxdb2 "github.com/influxdata/influxdb-client-go/v2"
	"github.com/influxdata/influxdb-client-go/v2/api"
	"github.com/influxdata/influxdb-client-go/v2/api/write"
)

// Writer batches points to one v2 bucket. It mirrors the Rust write path
// (line protocol, second-precision timestamps) without any v1 API.
type Writer struct {
	client influxdb2.Client
	write  api.WriteAPI
	org    string
	bucket string
}

// New builds a writer and starts the background error drain. Write errors
// surface ONLY through api.Errors() — failing to drain it loses them
// silently, so the drain is part of construction, not optional.
func New(url, token, org, bucket string) *Writer {
	client := influxdb2.NewClient(url, token)
	w := &Writer{client: client, write: client.WriteAPI(org, bucket), org: org, bucket: bucket}
	go func() {
		for err := range w.write.Errors() {
			slog.Error("influxdb write failed", "error", err)
		}
	}()
	return w
}

// Ping checks server reachability (mirrors Rust ping pre-check).
func (w *Writer) Ping(ctx context.Context) error {
	ok, err := w.client.Ping(ctx)
	if err != nil {
		return fmt.Errorf("influxdb ping failed: %w", err)
	}
	if !ok {
		return fmt.Errorf("influxdb ping unhealthy")
	}
	if _, err := w.client.Ready(ctx); err != nil {
		return fmt.Errorf("influxdb not ready: %w", err)
	}
	return nil
}

// Point mirrors one Rust measurement row: measurement + string tags +
// typed fields + explicit timestamp.
type Point struct {
	Measurement string
	Tags        map[string]string
	Fields      map[string]interface{}
	Time        time.Time
}

// Write enqueues a point on the async batcher. Nil field values are skipped
// (mirrors Rust LP omission of None).
func (w *Writer) Write(p Point) {
	fields := make(map[string]interface{}, len(p.Fields))
	for k, v := range p.Fields {
		if v == nil {
			continue
		}
		// Dereference common pointer scalars into plain values.
		switch t := v.(type) {
		case *float64:
			if t != nil {
				fields[k] = *t
			}
		case *int64:
			if t != nil {
				fields[k] = *t
			}
		case *bool:
			if t != nil {
				fields[k] = *t
			}
		case *string:
			if t != nil {
				fields[k] = *t
			}
		default:
			fields[k] = v
		}
	}
	w.write.WritePoint(write.NewPoint(p.Measurement, p.Tags, fields, p.Time))
}

// Flush blocks until queued points are sent. Call on shutdown.
func (w *Writer) Flush() {
	w.write.Flush()
}

// Close flushes and releases the client.
func (w *Writer) Close() {
	w.client.Close()
}
