//! Compact workload list items from kubectl `-o json` objects.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub fn item_age(creation: Option<&str>) -> String {
    let Some(raw) = creation else {
        return "—".into();
    };
    let Ok(created) = DateTime::parse_from_rfc3339(raw) else {
        return raw.to_string();
    };
    let created = created.with_timezone(&Utc);
    let secs = (Utc::now() - created).num_seconds().max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    format!("{days}d")
}

fn meta(obj: &Value) -> (&str, &str, Option<&str>) {
    let m = obj.get("metadata");
    let name = m
        .and_then(|x| x.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    let ns = m
        .and_then(|x| x.get("namespace"))
        .and_then(|x| x.as_str())
        .unwrap_or("default");
    let created = m
        .and_then(|x| x.get("creationTimestamp"))
        .and_then(|x| x.as_str());
    (name, ns, created)
}

fn container_images(containers: Option<&Value>) -> Vec<String> {
    containers
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.get("image").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn transform_namespace(obj: &Value) -> Value {
    let (name, _, created) = meta(obj);
    let phase = obj
        .get("status")
        .and_then(|s| s.get("phase"))
        .and_then(|p| p.as_str())
        .unwrap_or("Active");
    json!({
        "name": name,
        "status": phase,
        "age": item_age(created),
    })
}

pub fn transform_deployment(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let status = obj.get("status").cloned().unwrap_or(Value::Null);
    let spec = obj.get("spec").cloned().unwrap_or(Value::Null);
    let desired = spec
        .get("replicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let ready = status
        .get("readyReplicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let updated = status
        .get("updatedReplicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let available = status
        .get("availableReplicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let images = container_images(
        spec.get("template")
            .and_then(|t| t.get("spec"))
            .and_then(|s| s.get("containers")),
    );
    let label = if desired == 0 {
        "Stopped"
    } else if ready >= desired && available >= desired {
        "Running"
    } else if ready > 0 {
        "Progressing"
    } else {
        "Pending"
    };
    let selector = spec
        .get("selector")
        .and_then(|s| s.get("matchLabels"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "kind": "deployments",
        "name": name,
        "namespace": ns,
        "status": label,
        "ready": format!("{ready}/{desired}"),
        "updated": updated,
        "available": available,
        "images": images,
        "age": item_age(created),
        "replicas": desired,
        "selector": selector,
    })
}

pub fn transform_statefulset(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let status = obj.get("status").cloned().unwrap_or(Value::Null);
    let spec = obj.get("spec").cloned().unwrap_or(Value::Null);
    let desired = spec
        .get("replicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let ready = status
        .get("readyReplicas")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let images = container_images(
        spec.get("template")
            .and_then(|t| t.get("spec"))
            .and_then(|s| s.get("containers")),
    );
    let label = if desired == 0 {
        "Stopped"
    } else if ready >= desired {
        "Running"
    } else if ready > 0 {
        "Progressing"
    } else {
        "Pending"
    };
    json!({
        "kind": "statefulsets",
        "name": name,
        "namespace": ns,
        "status": label,
        "ready": format!("{ready}/{desired}"),
        "images": images,
        "age": item_age(created),
        "replicas": desired,
    })
}

pub fn transform_daemonset(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let status = obj.get("status").cloned().unwrap_or(Value::Null);
    let desired = status
        .get("desiredNumberScheduled")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let ready = status
        .get("numberReady")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let images = container_images(
        obj.get("spec")
            .and_then(|s| s.get("template"))
            .and_then(|t| t.get("spec"))
            .and_then(|s| s.get("containers")),
    );
    let label = if desired == 0 {
        "Pending"
    } else if ready >= desired {
        "Running"
    } else if ready > 0 {
        "Progressing"
    } else {
        "Pending"
    };
    json!({
        "kind": "daemonsets",
        "name": name,
        "namespace": ns,
        "status": label,
        "ready": format!("{ready}/{desired}"),
        "images": images,
        "age": item_age(created),
    })
}

pub fn transform_job(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let status = obj.get("status").cloned().unwrap_or(Value::Null);
    let succeeded = status
        .get("succeeded")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let failed = status
        .get("failed")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let active = status
        .get("active")
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    let completions = obj
        .get("spec")
        .and_then(|s| s.get("completions"))
        .and_then(|c| c.as_u64())
        .unwrap_or(1);
    let label = if succeeded >= completions {
        "Complete"
    } else if failed > 0 {
        "Failed"
    } else if active > 0 {
        "Running"
    } else {
        "Pending"
    };
    json!({
        "kind": "jobs",
        "name": name,
        "namespace": ns,
        "status": label,
        "ready": format!("{succeeded}/{completions}"),
        "age": item_age(created),
    })
}

pub fn transform_cronjob(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let spec = obj.get("spec").cloned().unwrap_or(Value::Null);
    let schedule = spec
        .get("schedule")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let suspend = spec
        .get("suspend")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let last = obj
        .get("status")
        .and_then(|s| s.get("lastScheduleTime"))
        .and_then(|t| t.as_str())
        .unwrap_or("—");
    let label = if suspend { "Suspended" } else { "Active" };
    json!({
        "kind": "cronjobs",
        "name": name,
        "namespace": ns,
        "status": label,
        "ready": "—",
        "schedule": schedule,
        "age": item_age(created),
        "lastSchedule": last,
    })
}

pub fn transform_pod(obj: &Value) -> Value {
    let (name, ns, created) = meta(obj);
    let status = obj.get("status").cloned().unwrap_or(Value::Null);
    let phase = status
        .get("phase")
        .and_then(|p| p.as_str())
        .unwrap_or("Unknown");
    let containers = status
        .get("containerStatuses")
        .and_then(|c| c.as_array());
    let (ready_n, total_n) = match containers {
        Some(arr) => {
            let total = arr.len() as u64;
            let ready = arr
                .iter()
                .filter(|c| c.get("ready").and_then(|r| r.as_bool()).unwrap_or(false))
                .count() as u64;
            (ready, total)
        }
        None => (0, 0),
    };
    let restarts = containers
        .map(|arr| {
            arr.iter()
                .map(|c| c.get("restartCount").and_then(|r| r.as_u64()).unwrap_or(0))
                .sum::<u64>()
        })
        .unwrap_or(0);
    let images = container_images(
        obj.get("spec")
            .and_then(|s| s.get("containers")),
    );
    let container_names: Vec<String> = obj
        .get("spec")
        .and_then(|s| s.get("containers"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let node = status
        .get("hostIP")
        .and_then(|h| h.as_str())
        .or_else(|| {
            obj.get("spec")
                .and_then(|s| s.get("nodeName"))
                .and_then(|n| n.as_str())
        })
        .unwrap_or("—");
    json!({
        "kind": "pods",
        "name": name,
        "namespace": ns,
        "status": phase,
        "ready": format!("{ready_n}/{total_n}"),
        "restarts": restarts,
        "images": images,
        "containers": container_names,
        "node": node,
        "age": item_age(created),
    })
}
