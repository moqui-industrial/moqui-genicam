# genicam_bridge.py
# Python bridge to communicate with GenICam vision cameras using Harvesters or a simulated Mock Camera.

import os
import json
import logging

# Configure logger
logger = logging.getLogger("genicam_bridge")
if not logger.handlers:
    handler = logging.StreamHandler()
    formatter = logging.Formatter('%(asctime)s - %(name)s - %(levelname)s - %(message)s')
    handler.setFormatter(formatter)
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)

# Try to import harvesters
try:
    from harvesters.core import Harvester
    HAS_HARVESTERS = True
    logger.info("Harvesters library loaded successfully. Real hardware communication enabled.")
except ImportError:
    HAS_HARVESTERS = False
    logger.warning("Harvesters library not found. Falling back to Mock Camera Simulation.")

# Path for Mock Camera State
MOCK_STATE_DIR = "runtime/genicam"
MOCK_STATE_FILE = os.path.join(MOCK_STATE_DIR, "mock_camera_state.json")

DEFAULT_MOCK_STATE = {
    "ExposureTime": 5000.0,
    "Gain": 0.0,
    "TriggerMode": "On",
    "TriggerSource": "Software",
    "TriggerSoftware": "Execute"
}

def _get_mock_state():
    """Helper to read mock camera state from file."""
    if not os.path.exists(MOCK_STATE_DIR):
        os.makedirs(MOCK_STATE_DIR)
    
    if not os.path.exists(MOCK_STATE_FILE):
        with open(MOCK_STATE_FILE, "w") as f:
            json.dump(DEFAULT_MOCK_STATE, f, indent=4)
        return DEFAULT_MOCK_STATE
    
    try:
        with open(MOCK_STATE_FILE, "r") as f:
            return json.load(f)
    except Exception as e:
        logger.error(f"Error reading mock camera state: {e}. Reverting to defaults.")
        return DEFAULT_MOCK_STATE

def _write_mock_state(state):
    """Helper to write mock camera state to file."""
    if not os.path.exists(MOCK_STATE_DIR):
        os.makedirs(MOCK_STATE_DIR)
    
    with open(MOCK_STATE_FILE, "w") as f:
        json.dump(state, f, indent=4)

def read_camera_parameters(cti_path, serial_number, parameter_names):
    """Reads current parameter values from camera."""
    logger.info(f"Reading parameters: {parameter_names} from camera {serial_number} using driver {cti_path}")
    
    if not HAS_HARVESTERS:
        # Mock mode
        mock_state = _get_mock_state()
        result = {}
        for name in parameter_names:
            result[name] = mock_state.get(name, None)
        logger.info(f"Mock read results: {result}")
        return result

    # Real Harvesters Mode
    h = Harvester()
    try:
        h.add_file(cti_path)
        h.update()
        
        # Connect by serial number
        # Harvesters select device matching criteria
        ia = h.create_image_acquirer(serial_number=serial_number)
        try:
            device = ia.device
            nodemap = device.node_map
            
            result = {}
            for name in parameter_names:
                if hasattr(nodemap, name):
                    node = getattr(nodemap, name)
                    # Read the node value (could be float, int, string, enum)
                    result[name] = node.value
                else:
                    logger.warning(f"Parameter {name} not found in camera node map.")
                    result[name] = None
            return result
        finally:
            ia.destroy()
    finally:
        h.reset()

def write_camera_parameters(cti_path, serial_number, parameters_map):
    """Writes target parameter values to camera."""
    logger.info(f"Writing parameters: {parameters_map} to camera {serial_number} using driver {cti_path}")
    
    if not HAS_HARVESTERS:
        # Mock mode
        mock_state = _get_mock_state()
        result = {}
        for name, value in parameters_map.items():
            # Support execute command simulation
            if name == "TriggerSoftware":
                logger.info("Executing SOFTWARE TRIGGER shot command (mock capture).")
                # Just confirm execution by leaving it or updating
                mock_state[name] = "Executed"
                result[name] = "Executed"
            else:
                # Convert value to correct type based on default value
                default_val = DEFAULT_MOCK_STATE.get(name)
                if isinstance(default_val, float) and value is not None:
                    try:
                        value = float(value)
                    except ValueError:
                        pass
                elif isinstance(default_val, int) and value is not None:
                    try:
                        value = int(value)
                    except ValueError:
                        pass
                
                mock_state[name] = value
                result[name] = value
        
        _write_mock_state(mock_state)
        logger.info(f"Mock write results: {result}")
        return result

    # Real Harvesters Mode
    h = Harvester()
    try:
        h.add_file(cti_path)
        h.update()
        
        ia = h.create_image_acquirer(serial_number=serial_number)
        try:
            device = ia.device
            nodemap = device.node_map
            
            result = {}
            for name, value in parameters_map.items():
                if hasattr(nodemap, name):
                    node = getattr(nodemap, name)
                    # Node is a command or parameter
                    # If it's a command node, we execute it
                    if hasattr(node, 'execute') and callable(getattr(node, 'execute')):
                        node.execute()
                        result[name] = "Executed"
                    else:
                        node.value = value
                        result[name] = node.value
                else:
                    logger.warning(f"Parameter {name} not found in camera node map.")
                    result[name] = None
            return result
        finally:
            ia.destroy()
    finally:
        h.reset()

def acquire_video_stream(cti_path, serial_number, num_frames=10, output_dir="runtime/genicam/frames"):
    """Starts acquisition, fetches a number of frames (simulated or real), and stops acquisition."""
    logger.info(f"Acquiring video stream of {num_frames} frames from camera {serial_number} using driver {cti_path}")
    
    if not HAS_HARVESTERS:
        # Mock video stream
        import time
        if not os.path.exists(output_dir):
            os.makedirs(output_dir)
        
        logger.info("Mock video acquisition started.")
        mock_files = []
        for i in range(num_frames):
            time.sleep(0.1) # Simulate frame time
            frame_path = os.path.join(output_dir, f"frame_{i:04d}.jpg")
            with open(frame_path, "w") as f:
                f.write(f"MOCK FRAME {i} DATA")
            mock_files.append(frame_path)
            logger.info(f"Mock acquired frame {i} saved to {frame_path}")
        logger.info("Mock video acquisition stopped.")
        return {"acquired_frames": mock_files}

    # Real Harvesters Mode
    h = Harvester()
    try:
        h.add_file(cti_path)
        h.update()
        
        ia = h.create_image_acquirer(serial_number=serial_number)
        try:
            if not os.path.exists(output_dir):
                os.makedirs(output_dir)
            
            # Start image acquisition
            ia.start()
            logger.info("Image acquisition started.")
            
            frame_files = []
            for i in range(num_frames):
                # Fetch a buffer (block until a frame is ready)
                with ia.fetch() as buffer:
                    component = buffer.payload.components[0]
                    width = component.width
                    height = component.height
                    data = component.data
                    
                    frame_path = os.path.join(output_dir, f"frame_{i:04d}.bin")
                    with open(frame_path, "wb") as f:
                        f.write(data)
                    frame_files.append(frame_path)
                    logger.info(f"Acquired frame {i} ({width}x{height}) saved to {frame_path}")
            
            # Stop image acquisition
            ia.stop()
            logger.info("Image acquisition stopped.")
            return {"acquired_frames": frame_files}
        finally:
            ia.destroy()
    finally:
        h.reset()

def run_action(action, cti_path, serial_number, parameter_names=None, parameters_map=None):
    """Router function called from JEP."""
    if action == "read":
        return read_camera_parameters(cti_path, serial_number, parameter_names or [])
    elif action == "write":
        return write_camera_parameters(cti_path, serial_number, parameters_map or {})
    elif action == "video":
        num_frames = parameters_map.get("num_frames", 10) if parameters_map else 10
        output_dir = parameters_map.get("output_dir", "runtime/genicam/frames") if parameters_map else "runtime/genicam/frames"
        return acquire_video_stream(cti_path, serial_number, num_frames=num_frames, output_dir=output_dir)
    else:
        raise ValueError(f"Unknown action: {action}")
