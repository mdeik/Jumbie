#!/usr/bin/env python3
"""Dummy Python plugin for testing the JSON-RPC IPC plugin system.

This is a reference example matching the plugin specification. It demonstrates:
  - IPC auth via JUMBIE_PLUGIN_SECRET
  - Auth echoing in every response
  - Params null-safety (normalized at read time)
  - All required base methods + MetadataProvider capability methods
"""

import json
import os
import sys

JUMBIE_SECRET = os.environ.get("JUMBIE_PLUGIN_SECRET")

# One process serves ALL instances of this plugin type: instance_id → config
# (populated by `set_config`). Config is input — there is no per-instance
# plugin object and no reconfigure logic.
instances = {}


def send_response(req_id, result, auth=None):
    if req_id is None:
        return  # Notification — no response expected
    resp = {"jsonrpc": "2.0", "result": result, "id": req_id}
    if auth:
        resp["auth"] = auth
    print(json.dumps(resp), flush=True)


def send_error(req_id, code, message, auth=None):
    if req_id is None:
        return
    resp = {
        "jsonrpc": "2.0",
        "error": {"code": code, "message": message},
        "id": req_id,
    }
    if auth:
        resp["auth"] = auth
    print(json.dumps(resp), flush=True)


def main():
    while True:
        line = sys.stdin.readline()
        if not line:
            break

        line = line.strip()
        if not line:
            continue

        # Defaults so error responses can echo them even when parsing failed.
        req_id = None
        req_auth = None

        try:
            req = json.loads(line)
            req_id = req.get("id")
            method = req.get("method")
            instance_id = req.get("instance_id")  # None for type-level methods
            # Normalise null/missing params to {} SSoT — all handlers below
            # can safely call .get() without None-guards.
            raw_params = req.get("params")
            params = raw_params if isinstance(raw_params, dict) else {}
            req_auth = req.get("auth")  # Echo back in responses

            # ── Auth check (optional, mirrors PluginServer behaviour) ─────
            if JUMBIE_SECRET and req_auth != JUMBIE_SECRET:
                send_error(req_id, -32001, "Authentication failed", req_auth)
                continue

            if method == "get_info":
                send_response(
                    req_id,
                    {
                        "display_name": "Dummy Python",
                        "version": "1.0.0",
                        "author": "System",
                        "description": "Dummy Python IPC test plugin",
                        "capabilities": ["metadata_provider"],
                        "series_identifier_label": "Series ID",
                        "series_identifier_placeholder": "e.g. 121361",
                        "rate_limit": {"requests_per_minute": 60, "burst": 5},
                        "supports_test": True,
                    },
                    req_auth,
                )
            elif method == "get_config_schema":
                schema = {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "title": "DummyConfig",
                    "type": "object",
                    "required": ["api_key"],
                    "properties": {
                        "api_key": {
                            "type": "string",
                            "description": "Your API Key",
                        },
                        "enable_logging": {
                            "type": "boolean",
                            "default": False,
                        },
                    },
                }
                send_response(req_id, schema, req_auth)
            elif method == "validate_config":
                api_key = params.get("api_key", "")
                if len(api_key) < 5:
                    send_response(
                        req_id,
                        {"errors": ["API key must be at least 5 characters"]},
                        req_auth,
                    )
                else:
                    send_response(req_id, True, req_auth)
            # ── Startup handshake (required — see spec §3) ───────────────
            # `hello` must echo the protocol version; without it the host
            # rejects the process at startup.
            elif method == "hello":
                send_response(req_id, {"protocol_version": 3}, req_auth)
            # ── Instance lifecycle (config is input) ──────────────────────
            # `set_config` is idempotent: it creates OR updates an instance's
            # config. There is no "reconfigure" method.
            elif method == "set_config":
                instances[instance_id] = params
                send_response(req_id, True, req_auth)
            elif method == "shutdown_instance":
                instances.pop(instance_id, None)
                send_response(req_id, True, req_auth)
            elif method == "test":
                # The Test button forces a live check; success clears any
                # failure cooldown.
                send_response(req_id, "Successfully connected to Dummy Python", req_auth)
            elif method == "health_check":
                send_response(req_id, "ok", req_auth)
            elif method == "fetch_series_metadata":
                # Return dummy SeriesMetadata matching the spec's shape
                send_response(
                    req_id,
                    {
                        "episodes": [
                            {
                                "unique_id": "1",
                                "season": 1,
                                "episode": 1,
                                "title": "Episode 1",
                                "description": None,
                                "runtime": None,
                                "image_url": None,
                                "meta_date": None,
                            }
                        ],
                        "seasons": [{"season": 1, "episode_count": 12}],
                    },
                    req_auth,
                )
            elif method == "fetch_series_info":
                send_response(
                    req_id,
                    {
                        "name": "Dummy Series",
                        "overview": "A test series",
                        "original_country": None,
                        "aliases": {},
                    },
                    req_auth,
                )
            elif method == "fetch_series_aliases":
                send_response(req_id, {}, req_auth)
            elif method == "get_updated_series":
                send_response(req_id, [], req_auth)
            elif method == "uses_absolute_episode_numbering":
                send_response(req_id, False, req_auth)
            else:
                send_error(req_id, -32601, f"Method '{method}' not found", req_auth)

        except json.JSONDecodeError as e:
            print(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "error": {"code": -32700, "message": f"Parse error: {e}"},
                        "id": req_id,
                        "auth": req_auth,
                    }
                ),
                flush=True,
            )
        except Exception as e:
            # Echo the request id + auth so the host can route the error back to
            # the caller — a response with id None and no auth echo is dropped,
            # which turns a raised exception into a 30s caller timeout.
            print(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "error": {"code": -32603, "message": str(e)},
                        "id": req_id,
                        "auth": req_auth,
                    }
                ),
                flush=True,
            )


if __name__ == "__main__":
    main()
