# rfd-kube-init

A Kubernetes initialization tool that distributes secrets across namespaces. Currently supports Meilisearch tenant token generation.

## Overview

This tool is designed to run as a Kubernetes Job or init container. It conditionally initializes secrets based on which environment variables are configured:

- **Meilisearch**: If `MEILI_MASTER_NAMESPACE`, `MEILI_MASTER_SECRET_NAME`, and `MEILI_MASTER_SECRET_KEY` are set, reads the master key from Kubernetes, generates tenant tokens, and writes them to secrets in target namespaces

If a feature's required environment variables are not set (or empty), that feature is skipped.

## Meilisearch Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `MEILI_MASTER_NAMESPACE` | Yes* | Namespace containing the Meilisearch master key secret |
| `MEILI_MASTER_SECRET_NAME` | Yes* | Name of the secret containing the master key |
| `MEILI_MASTER_SECRET_KEY` | Yes* | Key within the secret that holds the master key value |
| `MEILI_HOST` | Yes | Meilisearch host URL (e.g., `http://meilisearch:7700`) |
| `MEILI_RW_TOKEN_TARGET_NAMESPACES` | Yes** | Comma-delimited list of namespaces for read-write tokens (e.g., `rfd-api`) |
| `MEILI_RO_TOKEN_TARGET_NAMESPACES` | Yes** | Comma-delimited list of namespaces for read-only tokens (e.g., `rfd-web`) |
| `MEILI_SECRET_NAME` | No | Name of the secret to create (default: `meilisearch-token`) |
| `MEILI_API_EXPIRATION_SECONDS` | No | Token expiration in seconds from now (default: no expiration) |
| `MEILI_TOKEN_FILTER` | No | JSON search rules for the tenant token (default: `["*"]` - full access to all indexes) |

*If any of `MEILI_MASTER_NAMESPACE`, `MEILI_MASTER_SECRET_NAME`, or `MEILI_MASTER_SECRET_KEY` are unset or empty, Meilisearch initialization is skipped entirely.

**At least one of `MEILI_RW_TOKEN_TARGET_NAMESPACES` or `MEILI_RO_TOKEN_TARGET_NAMESPACES` must be set.

## Token Types

The tool generates two types of tenant tokens:

- **RW (Read-Write)**: Generated from the "Default Admin API Key". Grants full access including document indexing, settings updates, and searches. Use for backend services that need to write to Meilisearch.

- **RO (Read-Only)**: Generated from the "Default Search API Key". Grants search access only. Use for frontend applications or services that only need to query.

## How It Works

1. Reads the Meilisearch master key from the specified Kubernetes secret
2. Connects to Meilisearch and fetches the list of API keys
3. For RW namespaces: finds the "Default Admin API Key" and generates a tenant token
4. For RO namespaces: finds the "Default Search API Key" and generates a tenant token
5. Writes the appropriate token to secrets in each target namespace

## Secret Format

The tool creates an `Opaque` secret with the following data:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: meilisearch-token  # or MEILI_SECRET_NAME
type: Opaque
stringData:
  MEILISEARCH_API_KEY: <generated-tenant-token>
```

## Kubernetes RBAC

The service account running this tool needs permissions to:
- **Read** secrets in the namespace containing the master key
- **Create/patch** secrets in target namespaces

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: rfd-kube-init
  namespace: rfd-system
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: rfd-kube-init
rules:
  - apiGroups: [""]
    resources: ["secrets"]
    verbs: ["get", "create", "patch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: rfd-kube-init
subjects:
  - kind: ServiceAccount
    name: rfd-kube-init
    namespace: rfd-system
roleRef:
  kind: ClusterRole
  name: rfd-kube-init
  apiGroup: rbac.authorization.k8s.io
```

For namespace-scoped permissions, use `Role` and `RoleBinding` instead. Note that you'll need read access in the source namespace and write access in target namespaces.

## Example: Kubernetes Job

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: meilisearch-token-init
  namespace: rfd-system
spec:
  template:
    spec:
      serviceAccountName: rfd-kube-init
      restartPolicy: OnFailure
      containers:
        - name: init
          image: ghcr.io/oxidecomputer/rfd-kube-init:latest
          env:
            # Source of master key
            - name: MEILI_MASTER_NAMESPACE
              value: "meilisearch"
            - name: MEILI_MASTER_SECRET_NAME
              value: "meilisearch-master"
            - name: MEILI_MASTER_SECRET_KEY
              value: "MEILI_MASTER_KEY"
            # Meilisearch connection
            - name: MEILI_HOST
              value: "http://meilisearch.meilisearch:7700"
            # Token distribution
            - name: MEILI_RW_TOKEN_TARGET_NAMESPACES
              value: "rfd-api,rfd-processor"
            - name: MEILI_RO_TOKEN_TARGET_NAMESPACES
              value: "rfd-web"
            - name: MEILI_API_EXPIRATION_SECONDS
              value: "86400"  # 24 hours
```

## Example: CronJob for Token Rotation

```yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: meilisearch-token-refresh
  namespace: rfd-system
spec:
  schedule: "0 0 * * *"  # Daily at midnight
  jobTemplate:
    spec:
      template:
        spec:
          serviceAccountName: rfd-kube-init
          restartPolicy: OnFailure
          containers:
            - name: init
              image: ghcr.io/oxidecomputer/rfd-kube-init:latest
              env:
                - name: MEILI_MASTER_NAMESPACE
                  value: "meilisearch"
                - name: MEILI_MASTER_SECRET_NAME
                  value: "meilisearch-master"
                - name: MEILI_MASTER_SECRET_KEY
                  value: "MEILI_MASTER_KEY"
                - name: MEILI_HOST
                  value: "http://meilisearch.meilisearch:7700"
                - name: MEILI_RW_TOKEN_TARGET_NAMESPACES
                  value: "rfd-api,rfd-processor"
                - name: MEILI_RO_TOKEN_TARGET_NAMESPACES
                  value: "rfd-web"
                - name: MEILI_API_EXPIRATION_SECONDS
                  value: "90000"  # 25 hours (overlap for safety)
```

## Token Filter Examples

The `MEILI_TOKEN_FILTER` environment variable accepts a JSON value defining search rules. By default, the token grants access to all indexes with no filtering.

### Full access (default)
```bash
MEILI_TOKEN_FILTER='["*"]'
```

### Restrict to specific indexes
```bash
MEILI_TOKEN_FILTER='{"rfd_index": null, "other_index": null}'
```

### Restrict with filters
```bash
MEILI_TOKEN_FILTER='{"rfd_index": {"filter": "public = true"}}'
```

See [Meilisearch Tenant Tokens documentation](https://www.meilisearch.com/docs/learn/security/tenant_tokens) for full search rules syntax.

## Error Handling

- If any namespace write fails, the tool logs to stderr and continues processing remaining namespaces
- The tool exits with code 1 if any operation failed, code 0 if all succeeded
- Check Job/Pod logs for detailed error messages
