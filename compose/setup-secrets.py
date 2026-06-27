#!/usr/bin/env python3

import argparse
import json
import shlex
import subprocess
import sys
from pathlib import Path

AWS_SECRETS = {
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
}

GENERATED_SECRETS = {
    "JWT_PRIVATE_KEY",
    "JWT_PUBLIC_KEY",
    "SESSION_SECRET",
}

SECRETS = {
    "AWS_REGION",
    "GITHUB_HOST",
    "GITHUB_REPO_CLIENT_ID",
    "GITHUB_REPO_CLIENT_SECRET",
    "RFD_GITHUB_APP_ID",
    "RFD_GITHUB_INSTALLATION_ID",
    "RFD_GITHUB_OWNER",
    "RFD_GITHUB_REPO",
    "RFD_GITHUB_PATH",
    "RFD_GITHUB_DEFAULT_BRANCH",
    "S3_BUCKET",
    "MEILI_MASTER_KEY",
}

SECRETS_FROM_FILE = {
    "RFD_GITHUB_PRIVATE_KEY",
}

All_SECRETS = AWS_SECRETS.union(GENERATED_SECRETS, SECRETS, SECRETS_FROM_FILE)


class SetupSecretsError(Exception):
    pass


def run(*args, input_text=None, env=None, stdout=subprocess.DEVNULL, stderr=None):
    return subprocess.run(
        args,
        input=None if input_text is None else input_text.encode(),
        env=env,
        stdout=stdout,
        stderr=stderr,
        check=False,
    )


def checked_run(
    *args, input_text=None, env=None, stdout=subprocess.DEVNULL, stderr=None
):
    result = run(*args, input_text=input_text, env=env, stdout=stdout, stderr=stderr)
    if result.returncode != 0:
        raise SetupSecretsError(f"command failed: {shlex.join(args)}")
    return result


def load_existing_secrets():
    result = checked_run(
        "podman",
        "secret",
        "ls",
        "--format",
        "{{.Name}}",
        stdout=subprocess.PIPE,
    )
    return frozenset(
        line.strip() for line in result.stdout.decode().splitlines() if line.strip()
    )


def create_secret(name, value):
    checked_run("podman", "secret", "create", name, "-", input_text=value)


def create_secret_from_file(name, path):
    checked_run("podman", "secret", "create", name, str(path))


def replace_secret(name, value):
    checked_run("podman", "secret", "create", "--replace", name, "-", input_text=value)
    print(f"✓ Refreshed secret '{name}'.")


def validate_secret_value(name, value):
    if (
        name in {"RFD_GITHUB_APP_ID", "RFD_GITHUB_INSTALLATION_ID"}
        and not value.isdecimal()
    ):
        raise SetupSecretsError(f"  Error: '{name}' must be a numeric GitHub id.")


def delete_secret(name):
    checked_run("podman", "secret", "rm", name)
    print(f"✓ Removed secret '{name}'.")


def delete_secrets(existing_secrets, secrets_to_prune):
    requested_secrets = set(secrets_to_prune)

    for name in sorted(requested_secrets.difference(existing_secrets)):
        print(f"✓ Secret '{name}' is already missing.")

    existing_pruned_secrets = requested_secrets.intersection(existing_secrets)
    if not existing_pruned_secrets:
        return

    print("The following secrets will be deleted:")
    for name in sorted(existing_pruned_secrets):
        print(f"  {name}")

    approval = input("Delete these secrets? [y/N]: ")
    if approval.lower() not in {"y", "yes"}:
        raise SetupSecretsError("Secret deletion aborted.")

    for name in sorted(existing_pruned_secrets):
        delete_secret(name)


def setup_secrets(existing_secrets, skip_aws_secrets=False):
    print("Using the following variables:")
    print("----------------------------------------------------------------------")
    print("\n".join(sorted(All_SECRETS)))
    print("----------------------------------------------------------------------")
    print()

    refresh_aws_creds(existing_secrets, skip_aws_secrets=skip_aws_secrets)
    print()

    jwt_secrets = {"JWT_PRIVATE_KEY", "JWT_PUBLIC_KEY"}
    if jwt_secrets.difference(existing_secrets):
        print("JWT key secrets are missing; generating them now.")
        refresh_jwt_keys(existing_secrets)
        print()

    session_secrets = {"SESSION_SECRET"}
    if session_secrets.difference(existing_secrets):
        print("Session secret is missing; generating it now.")
        refresh_session_secret(existing_secrets)
        print()

    for name in sorted(SECRETS.intersection(existing_secrets)):
        print(f"✓ Secret '{name}' already exists. Skipping.")

    for name in sorted(SECRETS.difference(existing_secrets)):
        print(f"✗ Secret '{name}' does not exist.")
        value = input(f"  Enter value for {name}: ")
        validate_secret_value(name, value)
        create_secret(name, value)
        print(f"  Successfully created secret '{name}'.")

    for name in sorted(SECRETS_FROM_FILE.intersection(existing_secrets)):
        print(f"✓ Secret '{name}' already exists. Skipping.")

    for name in sorted(SECRETS_FROM_FILE.difference(existing_secrets)):
        print(f"✗ Secret '{name}' does not exist.")
        path = Path(input(f"  Enter path to file for {name}: "))

        if not path.is_file():
            raise SetupSecretsError(f"  Error: '{path}' is not a file.")
        if path.stat().st_size == 0:
            raise SetupSecretsError(f"  Error: '{path}' is empty.")

        create_secret_from_file(name, path)
        print(f"  Successfully created secret '{name}' from '{path}'.")

    print()
    print("All secrets setup complete!")


def create_empty_aws_secrets(existing_secrets):
    for name in sorted(AWS_SECRETS.difference(existing_secrets)):
        # --ignore leaves existing values intact while ensuring compose can
        # reference the secret even when AWS/S3 setup is skipped. Podman secrets
        # must be greater than 0 bytes, so use a single space placeholder.
        checked_run("podman", "secret", "create", "--ignore", name, "-", input_text=" ")


def refresh_aws_creds(existing_secrets, skip_aws_secrets=False):
    missing_aws_secrets = AWS_SECRETS.difference(existing_secrets)

    if skip_aws_secrets:
        if missing_aws_secrets:
            print("Creating empty AWS/S3 secrets.")
            create_empty_aws_secrets(existing_secrets)
        return

    if not missing_aws_secrets:
        return

    result = run(
        "aws",
        "configure",
        "export-credentials",
        stdout=subprocess.PIPE,
        stderr=None,
    )
    if result.returncode != 0:
        raise SetupSecretsError("Error: aws configure export-credentials failed.")

    credentials = json.loads(result.stdout.decode())

    create_empty_aws_secrets(existing_secrets)

    print("Refreshing AWS Podman secrets from 'aws configure export-credentials'.")
    replace_secret("AWS_ACCESS_KEY_ID", credentials["AccessKeyId"])
    replace_secret("AWS_SECRET_ACCESS_KEY", credentials["SecretAccessKey"])
    replace_secret("AWS_SESSION_TOKEN", credentials["SessionToken"])


def refresh_session_secret(existing_secrets):
    result = checked_run("openssl", "rand", "-hex", "16", stdout=subprocess.PIPE)
    session_secret = result.stdout.decode().strip()

    if not session_secret or len(session_secret) != 32:
        raise SetupSecretsError("Error: failed to generate session secret.")

    print("Refreshing session Podman secret.")
    replace_secret("SESSION_SECRET", session_secret)


def refresh_jwt_keys(existing_secrets):
    private_result = checked_run(
        "openssl",
        "genpkey",
        "-algorithm",
        "RSA",
        "-pkeyopt",
        "rsa_keygen_bits:2048",
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    private_key = private_result.stdout.decode().rstrip("\n")

    public_result = checked_run(
        "openssl",
        "pkey",
        "-pubout",
        input_text=private_key,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    public_key = public_result.stdout.decode().rstrip("\n")

    if not private_key or not public_key:
        raise SetupSecretsError("Error: failed to generate JWT keypair.")

    print("Generated JWT private key")
    print("Generated JWT public key")
    print("Refreshing JWT Podman secrets.")
    replace_secret("JWT_PRIVATE_KEY", private_key)
    replace_secret("JWT_PUBLIC_KEY", public_key)


def build_parser():
    parser = argparse.ArgumentParser(
        description="Interactively create and refresh local Podman secrets.",
        epilog="With no option, the script interactively creates any missing Podman secrets.",
    )
    parser.add_argument(
        "--skip-aws-secrets",
        "--skip-aws",
        action="store_true",
        help="skip AWS/S3 secrets during initial setup",
    )
    parser.add_argument(
        "--refresh-aws-creds",
        action="store_true",
        help="refresh AWS Podman secrets from `aws configure export-credentials`",
    )
    parser.add_argument(
        "--refresh-jwt-keys",
        action="store_true",
        help="generate a new JWT RSA keypair and refresh JWT Podman secrets",
    )
    parser.add_argument(
        "--refresh-session-secret",
        action="store_true",
        help="generate a new session secret and refresh its Podman secret",
    )
    parser.add_argument(
        "--reset-secrets",
        nargs="+",
        metavar="NAME",
        choices=sorted(All_SECRETS),
        help="remove tracked Podman secrets, then prompt to recreate them",
    )
    parser.add_argument(
        "--delete-all-secrets",
        action="store_true",
        help="delete all tracked Podman secrets",
    )
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)

    try:
        existing_secrets = load_existing_secrets()
        secrets_to_prune = set()

        if args.delete_all_secrets:
            delete_secrets(existing_secrets, secrets_to_prune)
            sys.exit(1)

        if args.refresh_aws_creds:
            secrets_to_prune.update(AWS_SECRETS)

        if args.refresh_jwt_keys:
            secrets_to_prune.update({"JWT_PRIVATE_KEY", "JWT_PUBLIC_KEY"})

        if args.refresh_session_secret:
            secrets_to_prune.update({"SESSION_SECRET"})

        if args.reset_secrets is not None:
            secrets_to_prune.update(args.reset_secrets)

        delete_secrets(existing_secrets, secrets_to_prune)
        existing_secrets = load_existing_secrets()

        setup_secrets(existing_secrets, skip_aws_secrets=args.skip_aws_secrets)
    except SetupSecretsError as error:
        print(error, file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print(file=sys.stderr)
        return 130

    return 0


if __name__ == "__main__":
    sys.exit(main())
