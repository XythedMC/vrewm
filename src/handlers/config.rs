
use toml::{from_str, to_string};
use serde::{Serialize, Deserialize};
use anyhow::{Error, anyhow};
use std::fs::{read_to_string, create_dir_all, write};
use dirs::config_dir;
#[derive(Serialize, Deserialize, Debug)]
pub struct TreeWMConfig {
    pub main_modifier: String,
    pub gap: f64,
    pub focused_border_color: [u8; 3],
    pub unfocused_border_color: [u8; 3],
    pub main_monitor: u8,
    pub use_title_bar: bool,
    pub corner_rounding: f32,
    pub border_width: f32,
    pub hover_to_focus: bool
}

pub fn read_config() -> Result<TreeWMConfig, Error>{
    let config_path = config_dir()
        .ok_or_else(|| anyhow!("Config directory ($HOME/.config) doesn't exist"))?
        .join("treewm")
        .join("treewm.toml");

    let contents = read_to_string(config_path)?;
    Ok(from_str(contents.as_str())?)
}

pub fn create_config() -> anyhow::Result<()>{
    let result_path = config_dir()
        .ok_or_else(|| anyhow!("Home path couldn't be found"))?
        .join("treewm")
        .join("treewm.toml");

    let values = TreeWMConfig { 
        main_modifier: String::from("Ctrl"),
        gap: 80.0, 
        focused_border_color: [255, 255, 255],
        unfocused_border_color: [0, 0, 0],
        main_monitor: 0, 
        use_title_bar: true,
        corner_rounding: 32.0,
        border_width: 2.0,
        hover_to_focus: true,
    };
    let toml = to_string(&values).expect("Couldn't create toml values");
    let _ = create_dir_all(result_path.parent().ok_or_else(|| anyhow!("Parent path to config file path couldnt be found"))?);
    write(result_path, toml)?;
    Ok(())
}