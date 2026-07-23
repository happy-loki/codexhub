const { spawn } = require("node:child_process");
const waitOn = require("wait-on");
const electron = require("electron");

const port = process.env.VITE_PORT || "5173";
const devServerUrl = process.env.VITE_DEV_SERVER_URL || `http://127.0.0.1:${port}`;

waitOn({ resources: [devServerUrl] })
  .then(() => {
    const child = spawn(electron, ["."], {
      stdio: "inherit",
      env: {
        ...process.env,
        VITE_DEV_SERVER_URL: devServerUrl
      }
    });
    child.on("exit", (code) => process.exit(code ?? 0));
  })
  .catch((error) => {
    console.error(error);
    process.exit(1);
  });
