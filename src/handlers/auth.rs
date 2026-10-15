//! Authentication handlers: login form, login submit, logout.

use argon2::{password_hash::PasswordHash, password_hash::PasswordVerifier, Argon2};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::middleware::auth::{
    clear_session_cookie, generate_session_token, hash_session_token, make_session_cookie,
    SESSION_MAX_AGE,
};
use crate::models::{RegisterRequest, SetPasswordRequest, User};
use crate::AppState;

/// Login form data
#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

/// Render the login page — professional design inspired by TailAdmin template.
pub async fn login_form() -> Response {
    let mut resp = Response::new(Body::from(LOGIN_PAGE_HTML));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

/// Handle user registration (public endpoint).
pub async fn register(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RegisterRequest>,
) -> Response {
    if form.password != form.confirm {
        return (StatusCode::BAD_REQUEST, "Las contraseñas no coinciden").into_response();
    }
    if form.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, "Mínimo 8 caracteres").into_response();
    }

    // Hash with Argon2
    use argon2::{
        password_hash::{PasswordHasher, PasswordVerifier, SaltString},
        Argon2,
    };
    let salt = SaltString::generate(&mut rand::thread_rng());
    let password_hash = match Argon2::default().hash_password(form.password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Error al crear cuenta: {}", e),
            )
                .into_response()
        }
    };

    let result = sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind(&form.username)
        .bind(&password_hash)
        .execute(&state.db)
        .await;

    match result {
        Ok(_) => (StatusCode::OK, "Cuenta creada. Ahora inicia sesión.").into_response(),
        Err(e) => {
            if e.to_string().contains("UNIQUE") {
                (StatusCode::CONFLICT, "Ese usuario ya existe").into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Error en la base de datos",
                )
                    .into_response()
            }
        }
    }
}

/// Handle login form submission.
pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    form: Form<LoginForm>,
) -> Response {
    // Validate Origin/Referer when present (CSRF defense)
    if let Some(rejection) = check_login_origin(&headers) {
        return rejection;
    }

    // Look up user
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?",
    )
    .bind(&form.username)
    .fetch_optional(&state.db)
    .await;

    match user {
        Ok(Some(user)) => {
            // Verify password with Argon2
            use argon2::{password_hash::PasswordHash, PasswordVerifier};
            let parsed_hash = match PasswordHash::new(&user.password_hash) {
                Ok(h) => h,
                Err(_) => {
                    return (StatusCode::UNAUTHORIZED, "Credenciales inválidas").into_response();
                }
            };

            if Argon2::default()
                .verify_password(form.password.as_bytes(), &parsed_hash)
                .is_ok()
            {
                // Create session with opaque CSPRNG token
                let raw_token = generate_session_token();
                let row_id = hash_session_token(&raw_token, &state.config.session_secret);
                let now = chrono::Utc::now();
                let expires = now + chrono::Duration::seconds(SESSION_MAX_AGE as i64);

                let _ = sqlx::query(
                    r#"INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)"#,
                )
                .bind(&row_id)
                .bind(user.id)
                .bind(expires.to_rfc3339())
                .execute(&state.db)
                .await;

                // Redirect to dashboard with session cookie
                let mut response = http::Response::builder()
                    .status(302)
                    .header("Location", "/dashboard")
                    .header("Set-Cookie", make_session_cookie(&raw_token))
                    .body(Body::empty())
                    .expect("redirect response builder");
                response
            } else {
                // Dummy work to prevent timing attacks
                use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
                let _ = Argon2::default()
                    .hash_password(b"dummy", &SaltString::generate(&mut rand::thread_rng()))
                    .ok();
                (StatusCode::UNAUTHORIZED, "Usuario o contraseña incorrectos").into_response()
            }
        }
        Ok(None) => {
            // Same dummy work even for missing users
            use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
            let _ = Argon2::default()
                .hash_password(b"dummy", &SaltString::generate(&mut rand::thread_rng()))
                .ok();
            (StatusCode::UNAUTHORIZED, "Usuario o contraseña incorrectos").into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error en el servidor").into_response(),
    }
}

/// Handle logout — deletes session row from DB.
pub async fn logout(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let raw = match crate::middleware::auth::extract_session_cookie(&req) {
        Some(v) => v,
        None => return axum::response::Redirect::to("/login").into_response(),
    };

    let hashed = hash_session_token(&raw, &state.config.session_secret);
    let _ = sqlx::query("DELETE FROM sessions WHERE id = ? OR id = ?")
        .bind(&hashed)
        .bind(&raw)
        .execute(&state.db)
        .await;

    let mut response = Response::new(Body::from(LOGIN_PAGE_HTML));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        http::header::SET_COOKIE,
        HeaderValue::from_str(&clear_session_cookie()).unwrap(),
    );
    response
}

/// Handle set-password (admin setup / change).
pub async fn set_password(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetPasswordRequest>,
) -> Response {
    match form.validate() {
        Ok(_) => {}
        Err(e) => {
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    }

    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    let salt = SaltString::generate(&mut rand::thread_rng());
    let password_hash = match Argon2::default().hash_password(form.password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            )
                .into_response()
        }
    };

    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
        .bind(&password_hash)
        .bind("admin")
        .execute(&state.db)
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => {
            (StatusCode::OK, "Contraseña actualizada").into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, "No se encontró al usuario 'admin'").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", e)).into_response(),
    }
}

/// Validate Origin/Referer on POST /login (CSRF defense).
fn check_login_origin(headers: &HeaderMap) -> Option<Response> {
    let origin = headers.get(http::header::ORIGIN)?;
    let host = headers.get(http::header::HOST)?;

    let origin_str = origin.to_str().ok()?;
    let host_str = host.to_str().ok()?;

    if origin_str.contains(host_str) || host_str.contains(origin_str) {
        None
    } else {
        Some((StatusCode::FORBIDDEN, "Origen no autorizado").into_response())
    }
}

const LOGIN_PAGE_HTML: &str = r##"<!DOCTYPE html>
<html lang="es">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Study Platform — Iniciar sesión</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined&family=Hanken+Grotesk:wght@400;500;600;700&family=Manrope:wght@600;700&display=swap" rel="stylesheet">
    <script src="https://unpkg.com/htmx.org@1.9.12"></script>
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        // Stitch — "Synthetic Cognition" design tokens
        tailwind.config = {
            theme: {
                extend: {
                    colors: {
                        background: '#101419',
                        'surface-container-lowest': '#0a0e13',
                        'surface-container-low': '#181c21',
                        'surface-container': '#1c2025',
                        'surface-container-high': '#262a30',
                        'surface-container-highest': '#31353b',
                        'on-surface': '#e0e2ea',
                        'on-surface-variant': '#bbcac5',
                        outline: '#859490',
                        'outline-variant': '#3c4946',
                        primary: '#b5fff0',
                        'on-primary': '#003730',
                        'primary-container': '#5eead4',
                        'on-primary-container': '#00675b',
                        secondary: '#9ed753',
                        'on-secondary': '#203700',
                        'secondary-container': '#6b9f1f',
                        'on-secondary-container': '#1b2f00',
                        'surface-tint': '#4ddcc6',
                        'text-primary': '#f1f5f9',
                        'text-secondary': '#94a3b8',
                        'text-muted': '#64748b',
                        canvas: '#0b0f14',
                        'input-bg': '#111821',
                    },
                    fontFamily: {
                        manrope: ['"Manrope"', 'sans-serif'],
                        hanken: ['"Hanken Grotesk"', 'sans-serif'],
                    },
                    boxShadow: {
                        glow: '0 0 12px rgba(158, 215, 83, 0.25)',
                        'glow-hover': '0 0 20px rgba(158, 215, 83, 0.45)',
                    },
                },
            },
        };
    </script>
</head>
<body class="bg-background font-hanken text-on-surface antialiased min-h-screen flex items-center justify-center">
    <div class="w-full max-w-sm px-6" id="login-container">
        <!-- Logo -->
        <div class="text-center mb-8">
            <div class="flex items-center justify-center gap-2 mb-2">
                <span class="material-symbols-outlined text-2xl text-surface-tint">psychology</span>
                <h1 class="font-manrope text-2xl font-bold"><span class="text-secondary">Study</span> Platform</h1>
            </div>
            <p class="text-sm text-on-surface-variant">Tu espacio de productividad académica</p>
        </div>
        <!-- Card -->
        <div class="relative bg-surface-container/85 border border-outline-variant border-t-2 border-t-primary-container rounded-xl p-8 backdrop-blur-lg">
            <h2 class="font-manrope text-lg font-semibold text-on-surface">Inicia sesión</h2>
            <p class="mt-1 text-sm text-on-surface-variant">Ingresa tus credenciales para continuar</p>
            <form hx-post="/login" hx-swap="outerHTML" hx-target="#login-container" class="mt-6 space-y-4">
                <div>
                    <label for="username" class="block text-sm font-medium text-on-surface-variant mb-1">Usuario</label>
                    <input id="username" type="text" name="username" required autocomplete="username" placeholder="Tu nombre de usuario" class="w-full h-10 px-3 bg-surface-container border border-[#37485a] rounded text-on-surface placeholder:text-muted transition-colors focus:outline-none focus:ring-2 focus:ring-surface-tint focus:border-surface-tint focus:ring-offset-2 focus:ring-offset-background">
                </div>
                <div>
                    <label for="password" class="block text-sm font-medium text-on-surface-variant mb-1">Contraseña</label>
                    <input id="password" type="password" name="password" required autocomplete="current-password" placeholder="••••••••" class="w-full h-10 px-3 bg-surface-container border border-[#37485a] rounded text-on-surface placeholder:text-muted transition-colors focus:outline-none focus:ring-2 focus:ring-surface-tint focus:border-surface-tint focus:ring-offset-2 focus:ring-offset-background">
                </div>
                <div class="flex items-center justify-between text-sm">
                    <label class="flex items-center gap-2 cursor-pointer text-on-surface-variant">
                        <input type="checkbox" class="sr-only peer">
                        <span class="flex h-5 w-5 items-center justify-center rounded bg-surface-container border border-outline text-xs text-transparent transition-colors peer-focus:ring-2 peer-focus:ring-surface-tint peer-focus:ring-offset-2 peer-focus:ring-offset-background peer-checked:bg-secondary peer-checked:border-secondary peer-checked:text-on-secondary">&#10003;</span>
                        Mantener sesión
                    </label>
                    <a href="#" class="text-secondary hover:underline">¿Olvidaste tu contraseña?</a>
                </div>
                <button type="submit" class="w-full h-11 rounded-lg bg-secondary text-on-secondary font-medium shadow-glow transition-colors hover:shadow-glow-hover focus:outline-none focus:ring-2 focus:ring-surface-tint focus:ring-offset-2 focus:ring-offset-background">Iniciar sesión</button>
            </form>
        </div>
        <p class="mt-6 text-center text-sm text-outline">¿No tienes cuenta? <a href="/register" class="font-medium text-secondary hover:text-secondary/90">Regístrate aquí</a></p>
    </div>
</body>
</html>"##;
