# CLI Documentation

## Command Line Interface

The `fred-cal` application provides command line options for connecting to a CalDAV server. All credentials can be provided either as direct values or as paths to files containing the values.

### Required Arguments

- `--caldav-server <CALDAV_SERVER>`: CalDAV server URL (must start with `http://` or `https://`)
- `--username <USERNAME>`: Username for CalDAV authentication
- `--password <PASSWORD>`: Password for CalDAV authentication

### Environment Variables

All arguments can also be provided via environment variables:

- `CALDAV_SERVER`: CalDAV server URL
- `CALDAV_USERNAME`: Username
- `CALDAV_PASSWORD`: Password

### Usage Examples

#### Using File Paths (Recommended for Security)

```bash
fred-cal \
  --caldav-server /run/secrets/email/icloud/caldav_server \
  --username /run/secrets/email/icloud/address \
  --password /run/secrets/email/icloud/password
```

#### Using Direct Values

```bash
fred-cal \
  --caldav-server "https://caldav.icloud.com" \
  --username "user@example.com" \
  --password "your-password"
```

#### Using Environment Variables

```bash
export CALDAV_SERVER="/run/secrets/email/icloud/caldav_server"
export CALDAV_USERNAME="/run/secrets/email/icloud/address"
export CALDAV_PASSWORD="/run/secrets/email/icloud/password"
fred-cal
```

### How File Loading Works

When you provide an argument value:

1. The application checks if the value exists as a file path
2. If it does, the file contents are read and trimmed of whitespace
3. If not, the value is used directly as-is

This allows you to:

- Store secrets in files for better security
- Use direct values for testing or when files aren't available
- Mix and match (e.g., file for password, direct value for URL)

### Validation

The application validates all inputs:

- **CalDAV Server URL**: Must start with `http://` or `https://`, cannot be empty
- **Username**: Cannot be empty
- **Password**: Cannot be empty

If validation fails, the application will exit with an error message explaining what went wrong.

### Security Best Practices

1. **Never hardcode credentials**: Always use file paths or environment variables
2. **Restrict file permissions**: Ensure secret files are only readable by your user

   ```bash
   chmod 600 /run/secrets/email/icloud/*
   ```

3. **Use HTTPS**: Always connect to CalDAV servers over HTTPS in production
4. **Don't commit secrets**: Never commit secret files to version control

### Exit Codes

- `0`: Success
- `1`: Error (invalid arguments, connection failure, etc.)
