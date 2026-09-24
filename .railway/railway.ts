import { defineRailway, project, service, volume } from "railway/iac";

// This repository manages only its own resources in the environment. Other
// repositories export their own partial name.
// See https://docs.railway.com/infrastructure-as-code#multi-repo-projects
export const partial = "hiring-radar";

export default defineRailway(() => {
  const data_volume = volume("data");

  const hiring_radar = service("hiring-radar", {
    healthcheck: "/api/status",
    healthcheckTimeout: 100,
    dockerfilePath: "Dockerfile.railway",
    builder: "DOCKERFILE",
    volumes: [{ mountPath: "/data", volume: data_volume }],
    env: {
      RADAR_CONFIG: "/app/config.toml",
      RADAR_COMPANIES_DIR: "/app/companies",
      RADAR_UI_DIR: "/app/ui",
      RADAR_DB: "/data/hiring.db",
      RADAR_BIND: "0.0.0.0:8080",
      RUST_LOG: "hiring_radar=info,tower_http=warn",
    },
  });
  return project("courteous-appreciation", {
    resources: [hiring_radar, data_volume],
  });
});
