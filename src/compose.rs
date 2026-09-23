//! Compose operations compile into ordinary supervised commands.
use crate::model::Compose;

pub fn commands(compose: &Compose) -> (Vec<String>, Vec<String>) {
    let base = vec![
        "docker".into(),
        "compose".into(),
        "--project-name".into(),
        compose.project.clone(),
        "--file".into(),
        compose.file.clone(),
    ];
    let mut up = base.clone();
    up.extend([
        "up".into(),
        if compose.build {
            "--build".into()
        } else {
            "--no-build".into()
        },
        "--abort-on-container-exit".into(),
        "--exit-code-from".into(),
        compose.service.clone(),
        compose.service.clone(),
    ]);
    let mut down = base;
    down.push("down".into());
    (up, down)
}
