terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
  }
}

provider "google" {
  project     = "halogen-acumen-406418"
  region      = "us-central1"
  credentials = file("~/.config/gcloud/smelt-dev-key.json")
}

# --- VPC Network ---

resource "google_compute_network" "main" {
  name                    = "bench-tofu-vpc"
  auto_create_subnetworks = false
  routing_mode            = "REGIONAL"
}

# --- Subnet ---

resource "google_compute_subnetwork" "app" {
  name          = "bench-tofu-app-subnet"
  ip_cidr_range = "10.0.0.0/24"
  network       = google_compute_network.main.id
  region        = "us-central1"
}

# --- Service Account ---

resource "google_service_account" "app" {
  account_id   = "bench-tofu-app"
  display_name = "Benchmark application SA"
}

# --- Pub/Sub Topic ---

resource "google_pubsub_topic" "events" {
  name = "bench-tofu-events"
}

# --- Artifact Registry ---

resource "google_artifact_registry_repository" "app" {
  location      = "us-central1"
  repository_id = "bench-tofu-app"
  description   = "Application container images"
  format        = "DOCKER"
}

# --- BigQuery Dataset ---

resource "google_bigquery_dataset" "analytics" {
  dataset_id    = "bench_tofu_analytics"
  friendly_name = "Analytics"
  description   = "Analytics and reporting data"
  location      = "US"
}

# --- Log Metric ---

resource "google_logging_metric" "app_errors" {
  name        = "bench-tofu-app-errors"
  description = "Count of application errors"
  filter      = "severity >= ERROR"

  metric_descriptor {
    metric_kind = "DELTA"
    value_type  = "INT64"
  }
}
