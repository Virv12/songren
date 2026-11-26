# Songren — Requirements

## Functional Requirements

1. **Declarative Service Management**
   - Define each service's desired state (containers, reverse proxy, local DNS, public DNS) in a single declarative configuration.
   - Support adding, updating, and completely tearing down services when their configuration is added, modified, or removed.
   - Support both reconciling all changed configurations and force-updating a specific service (e.g., to pull a new `:latest` image without config changes).

2. **Automatic Networking & Variables**
   - Automatically resolve low-level networking details (such as container IP addresses or unused host ports) so service configs don't need manual port/IP management.
   - Ensure new and old instances get non-conflicting endpoints during updates to support zero-downtime side-by-side cutovers.

3. **Container Lifecycle**
   - Support **zero-downtime** updates by default: pull and start the new container, switch proxy traffic, then stop/remove the old container.
   - Support **stop-first (recreate)** updates for stateful or single-instance containers that cannot run two versions concurrently.

4. **Static Files (Git Repositories)**
   - Clone, update, and clean up Git repositories used for static file hosting (served via the reverse proxy).
   - Support updating a repository on demand (e.g., via CI/webhook on push) as well as during configuration sync.

5. **Reverse Proxy**
   - Automatically configure and reload the reverse proxy for each service (containers or static sites), validating the configuration before applying changes.

6. **Local & Public DNS**
   - Manage local DNS records for internal routing.
   - Manage public DNS records, including cleaning up removed or outdated records.

7. **Triggering & Usability**
   - Make updating and applying configuration changes simple and frictionless (e.g., triggered automatically on config push or via a single command/call).
   - Support authenticated remote triggering to force-update a specific service (e.g., from CI on `:latest` image pushes or static site repo pushes).

## Non-Functional Requirements

1. **Security**
   - Authenticate all API/webhook requests.
   - Validate and sanitize service identifiers.

2. **Reliability & Concurrency**
   - Prevent concurrent updates from racing on the same service or shared system reloads.
   - Abort safely on failure (e.g., invalid proxy config or failed container startup) without tearing down the currently running service.

3. **Portability & Operations**
   - Implemented in Rust and distributed as a signed system package (`.deb`) managed by `systemd`.
   - Support self-updating cleanly via the system package manager (on a schedule or remotely triggered).
   - Run cleanly in its target deployment environment without hidden host dependencies or missing-directory errors.
   - Provide clear logs for every step and automated tests for core reconciliation logic.
