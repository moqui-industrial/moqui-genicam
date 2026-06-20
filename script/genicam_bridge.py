# genicam_bridge.py
# Python bridge to communicate with GenICam vision cameras using Harvesters or a simulated Mock Camera.

import os
import json
import logging
import threading
import time

# Configure logger
logger = logging.getLogger("genicam_bridge")
if not logger.handlers:
    handler = logging.StreamHandler()
    formatter = logging.Formatter('%(asctime)s - %(name)s - %(levelname)s - %(message)s')
    handler.setFormatter(formatter)
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)

# Try to import OpenCV
try:
    import cv2
    import numpy as np
    HAS_OPENCV = True
    logger.info("OpenCV (cv2) library loaded successfully.")
except ImportError:
    HAS_OPENCV = False
    logger.warning("OpenCV (cv2) library not found. Falling back to raw byte handling.")

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
    "TriggerSoftware": "Execute",
    "AcquisitionStart": "Execute",
    "AcquisitionStop": "Execute",
    "LatestFrame": ""
}

# Global dictionary to cache active CameraConnection objects (keyed by (cti_path, serial_number))
_connections_cache = {}

# Thread-safe global cache for streaming
_latest_frames = {} # serial_number -> (jpeg_bytes, component_index, data_format)
_latest_frames_lock = threading.Lock()
_streaming_threads = {} # serial_number -> AcquisitionThread
_streaming_threads_lock = threading.Lock()

class CameraConnection:
    def __init__(self, cti_path, serial_number):
        self.cti_path = cti_path
        self.serial_number = serial_number
        self.harvester = None
        self.acquirer = None

    def connect(self):
        if self.acquirer is not None:
            return # Already connected!

        logger.info(f"Opening persistent connection to camera {self.serial_number} using driver {self.cti_path}")
        
        retry_count = 0
        backoff = 1.0
        
        while retry_count < 3:
            try:
                self.harvester = Harvester()
                self.harvester.add_file(self.cti_path)
                self.harvester.update()
                self.acquirer = self.harvester.create_image_acquirer(serial_number=self.serial_number)
                logger.info(f"Successfully connected to camera {self.serial_number}")
                return
            except Exception as e:
                retry_count += 1
                logger.warning(f"Connection attempt {retry_count}/3 failed for {self.serial_number}: {e}")
                self.disconnect()
                if retry_count < 3:
                    time.sleep(backoff)
                    backoff *= 2.0
        
        raise ConnectionError(f"Failed to connect to camera {self.serial_number} after 3 attempts.")

    def disconnect(self):
        logger.info(f"Closing connection to camera {self.serial_number}")
        if self.acquirer:
            try:
                self.acquirer.destroy()
            except Exception as e:
                logger.error(f"Error destroying acquirer: {e}")
            self.acquirer = None
        if self.harvester:
            try:
                self.harvester.reset()
            except Exception as e:
                logger.error(f"Error resetting harvester: {e}")
            self.harvester = None

def get_connection(cti_path, serial_number):
    """Retrieves or creates a cached connection to the camera."""
    key = (cti_path, serial_number)
    if key not in _connections_cache:
        _connections_cache[key] = CameraConnection(cti_path, serial_number)
    
    conn = _connections_cache[key]
    if HAS_HARVESTERS:
        conn.connect()
    return conn

def close_all_connections():
    """Closes all active camera connections (pool cleanup)."""
    # First stop all active streaming threads
    with _streaming_threads_lock:
        for serial, thread in list(_streaming_threads.items()):
            try:
                thread.stop()
            except Exception as e:
                logger.error(f"Error stopping streaming thread for {serial}: {e}")
        _streaming_threads.clear()

    for key, conn in list(_connections_cache.items()):
        try:
            conn.disconnect()
        except Exception as e:
            logger.error(f"Error during pool cleanup for {key}: {e}")
    _connections_cache.clear()

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

def _generate_mock_jpeg(serial_number, frame_index):
    """Generates a valid mock JPEG image using NumPy and OpenCV."""
    if not HAS_OPENCV:
        # Fallback to simple bytes
        return f"MOCK FRAME {frame_index} FOR CAMERA {serial_number} (OpenCV missing)".encode('utf-8')
    
    # Create black image 640x480
    img = np.zeros((480, 640, 3), dtype=np.uint8)
    
    # Draw simple colored panels/text
    cv2.putText(img, "Moqui GenICam Mock", (50, 100), cv2.FONT_HERSHEY_SIMPLEX, 1.0, (0, 255, 0), 2)
    cv2.putText(img, f"Camera: {serial_number}", (50, 180), cv2.FONT_HERSHEY_SIMPLEX, 0.8, (255, 255, 255), 2)
    cv2.putText(img, f"Frame ID: {frame_index}", (50, 260), cv2.FONT_HERSHEY_SIMPLEX, 0.8, (255, 255, 0), 2)
    
    import datetime
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
    cv2.putText(img, ts, (50, 340), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 255, 255), 1)
    
    success, encoded_img = cv2.imencode('.jpg', img)
    if success:
        return encoded_img.tobytes()
    return b""

def convert_to_numpy_array(component):
    """Converts a Harvesters payload component into a standard BGR image array using OpenCV."""
    if not HAS_OPENCV:
        return None
    
    try:
        width = component.width
        height = component.height
        data = component.data # NumPy array (1D)
        
        # Safe access to component data format
        data_format = "Mono8"
        if hasattr(component, 'data_format_name'):
            data_format = component.data_format_name
        elif hasattr(component, 'data_format'):
            data_format = str(component.data_format)
            
        logger.info(f"Converting component format: {data_format} ({width}x{height})")
        
        if data_format == "Mono8":
            return data.reshape(height, width)
        elif data_format == "RGB8":
            img = data.reshape(height, width, 3)
            return cv2.cvtColor(img, cv2.COLOR_RGB2BGR)
        elif data_format == "BGR8":
            return data.reshape(height, width, 3)
        elif data_format in ("BayerRG8", "BayerRG"):
            img = data.reshape(height, width)
            return cv2.cvtColor(img, cv2.COLOR_BayerRG2BGR)
        elif data_format in ("BayerGR8", "BayerGR"):
            img = data.reshape(height, width)
            return cv2.cvtColor(img, cv2.COLOR_BayerGR2BGR)
        elif data_format in ("BayerGB8", "BayerGB"):
            img = data.reshape(height, width)
            return cv2.cvtColor(img, cv2.COLOR_BayerGB2BGR)
        elif data_format in ("BayerBG8", "BayerBG"):
            img = data.reshape(height, width)
            return cv2.cvtColor(img, cv2.COLOR_BayerBG2BGR)
        else:
            logger.warning(f"Unsupported format {data_format}. Returning grayscale if size matches.")
            if len(data) == width * height:
                return data.reshape(height, width)
            return None
    except Exception as e:
        logger.error(f"Error during pixel format conversion: {e}")
        return None

class AcquisitionThread(threading.Thread):
    def __init__(self, cti_path, serial_number):
        super().__init__(name=f"AcqThread-{serial_number}")
        self.cti_path = cti_path
        self.serial_number = serial_number
        self.running = False
        self.daemon = True
        self.mock_frame_index = 0

    def run(self):
        self.running = True
        logger.info(f"Background acquisition thread started for camera {self.serial_number}")
        
        retry_count = 0
        backoff = 1.0
        
        while self.running:
            if not HAS_HARVESTERS:
                # Mock acquisition loop
                try:
                    time.sleep(0.1) # Simulate 10 FPS
                    self.mock_frame_index += 1
                    jpeg_bytes = _generate_mock_jpeg(self.serial_number, self.mock_frame_index)
                    with _latest_frames_lock:
                        _latest_frames[self.serial_number] = (jpeg_bytes, 0, "Mono8")
                except Exception as e:
                    logger.error(f"Error in mock acquisition loop: {e}")
                continue
            
            # Real Harvesters acquisition loop
            try:
                conn = get_connection(self.cti_path, self.serial_number)
                ia = conn.acquirer
                
                if not ia.is_acquiring():
                    ia.start()
                    logger.info(f"Image acquirer started in background thread for {self.serial_number}")
                
                retry_count = 0  # Reset retry counter on successful connection/acquisition start
                backoff = 1.0
                
                while self.running:
                    try:
                        # Fetch buffer with short timeout to keep thread responsive to stop signals
                        with ia.fetch(timeout=1.0) as buffer:
                            payload = buffer.payload
                            components = getattr(payload, 'components', [])
                            
                            if not components:
                                continue
                            
                            # Standard component resolution
                            comp = components[0]
                            img = convert_to_numpy_array(comp)
                            
                            if img is not None and HAS_OPENCV:
                                success, encoded_img = cv2.imencode('.jpg', img)
                                if success:
                                    jpeg_bytes = encoded_img.tobytes()
                                    with _latest_frames_lock:
                                        _latest_frames[self.serial_number] = (jpeg_bytes, 0, getattr(comp, 'data_format_name', 'Mono8'))
                            else:
                                with _latest_frames_lock:
                                    _latest_frames[self.serial_number] = (comp.data.tobytes(), 0, getattr(comp, 'data_format_name', 'Mono8'))
                    except Exception as e:
                        # Timeout exceptions are expected when frame rates are low
                        if "Timeout" in type(e).__name__ or "timeout" in str(e).lower():
                            continue
                        else:
                            raise e
            except Exception as e:
                logger.error(f"Acquisition error for camera {self.serial_number}: {e}")
                if self.running:
                    retry_count += 1
                    if retry_count > 3:
                        logger.error(f"Max connection retries reached for {self.serial_number}. Stopping streaming thread.")
                        self.running = False
                        break
                    
                    logger.info(f"Attempting reconnect {retry_count}/3 for {self.serial_number} in {backoff}s...")
                    time.sleep(backoff)
                    backoff *= 2.0
                    
                    try:
                        key = (self.cti_path, self.serial_number)
                        if key in _connections_cache:
                            _connections_cache[key].disconnect()
                    except:
                        pass
        
        logger.info(f"Background acquisition thread stopped for camera {self.serial_number}")
        if HAS_HARVESTERS:
            try:
                key = (self.cti_path, self.serial_number)
                if key in _connections_cache:
                    conn = _connections_cache[key]
                    if conn.acquirer and conn.acquirer.is_acquiring():
                        conn.acquirer.stop()
            except Exception as e:
                logger.error(f"Error stopping acquirer: {e}")

    def stop(self):
        self.running = False

def _start_stream(cti_path, serial_number):
    """Starts the background streaming acquisition thread."""
    with _streaming_threads_lock:
        if serial_number not in _streaming_threads:
            thread = AcquisitionThread(cti_path, serial_number)
            thread.start()
            _streaming_threads[serial_number] = thread
            logger.info(f"Streaming started for camera {serial_number}")

def _stop_stream(serial_number):
    """Stops the background streaming acquisition thread."""
    with _streaming_threads_lock:
        if serial_number in _streaming_threads:
            thread = _streaming_threads[serial_number]
            thread.stop()
            thread.join(timeout=3.0)
            del _streaming_threads[serial_number]
            logger.info(f"Streaming stopped for camera {serial_number}")

def _write_latest_frame_file(serial_number):
    """Writes the latest cached frame bytes to disk and returns its file path."""
    with _latest_frames_lock:
        frame_data = _latest_frames.get(serial_number, None)
    
    if not frame_data:
        # If no frame is cached, generate a default one
        jpeg_bytes = _generate_mock_jpeg(serial_number, 0)
    else:
        jpeg_bytes = frame_data[0]
        
    frames_dir = "runtime/genicam/frames"
    os.makedirs(frames_dir, exist_ok=True)
    frame_path = os.path.join(frames_dir, f"latest_{serial_number}.jpg")
    
    with open(frame_path, "wb") as f:
        f.write(jpeg_bytes)
        
    return frame_path

def read_camera_parameters(cti_path, serial_number, parameter_names):
    """Reads current parameter values from camera."""
    logger.info(f"Reading parameters: {parameter_names} from camera {serial_number}")
    
    result = {}
    for name in parameter_names:
        if name == "LatestFrame":
            # Save the latest cached frame to a file and return the path
            result[name] = _write_latest_frame_file(serial_number)
            continue
            
        if not HAS_HARVESTERS:
            # Mock mode
            mock_state = _get_mock_state()
            result[name] = mock_state.get(name, None)
        else:
            # Real mode
            conn = get_connection(cti_path, serial_number)
            try:
                device = conn.acquirer.device
                nodemap = device.node_map
                if hasattr(nodemap, name):
                    node = getattr(nodemap, name)
                    result[name] = node.value
                else:
                    logger.warning(f"Parameter {name} not found in camera node map.")
                    result[name] = None
            except Exception as e:
                logger.error(f"Error reading parameter {name}: {e}. Retrying after reset.")
                conn.disconnect()
                conn = get_connection(cti_path, serial_number)
                nodemap = conn.acquirer.device.node_map
                if hasattr(nodemap, name):
                    result[name] = getattr(nodemap, name).value
                else:
                    result[name] = None
                    
    logger.info(f"Read results: {result}")
    return result

def write_camera_parameters(cti_path, serial_number, parameters_map):
    """Writes target parameter values to camera."""
    logger.info(f"Writing parameters: {parameters_map} to camera {serial_number}")
    
    # Intercept streaming control commands
    if "AcquisitionStart" in parameters_map:
        _start_stream(cti_path, serial_number)
    if "AcquisitionStop" in parameters_map:
        _stop_stream(serial_number)
        
    result = {}
    for name, value in parameters_map.items():
        if not HAS_HARVESTERS:
            # Mock mode
            mock_state = _get_mock_state()
            if name == "TriggerSoftware":
                logger.info("Executing SOFTWARE TRIGGER shot command (mock capture).")
                mock_state[name] = "Executed"
                result[name] = "Executed"
            else:
                default_val = DEFAULT_MOCK_STATE.get(name)
                if isinstance(default_val, float) and value is not None:
                    try: value = float(value)
                    except ValueError: pass
                elif isinstance(default_val, int) and value is not None:
                    try: value = int(value)
                    except ValueError: pass
                
                mock_state[name] = value
                result[name] = value
            _write_mock_state(mock_state)
        else:
            # Real mode
            conn = get_connection(cti_path, serial_number)
            try:
                device = conn.acquirer.device
                nodemap = device.node_map
                if hasattr(nodemap, name):
                    node = getattr(nodemap, name)
                    if hasattr(node, 'execute') and callable(getattr(node, 'execute')):
                        node.execute()
                        result[name] = "Executed"
                    else:
                        node.value = value
                        result[name] = node.value
                else:
                    logger.warning(f"Parameter {name} not found in camera node map.")
                    result[name] = None
            except Exception as e:
                logger.error(f"Error writing parameter {name}: {e}. Retrying after reset.")
                conn.disconnect()
                conn = get_connection(cti_path, serial_number)
                nodemap = conn.acquirer.device.node_map
                if hasattr(nodemap, name):
                    node = getattr(nodemap, name)
                    if hasattr(node, 'execute') and callable(getattr(node, 'execute')):
                        node.execute()
                        result[name] = "Executed"
                    else:
                        node.value = value
                        result[name] = node.value
                else:
                    result[name] = None
                    
    logger.info(f"Write results: {result}")
    return result

def acquire_video_stream(cti_path, serial_number, num_frames=10, output_dir="runtime/genicam/frames"):
    """Starts acquisition, fetches a number of frames, and stops acquisition."""
    logger.info(f"Acquiring video stream of {num_frames} frames from camera {serial_number}")
    os.makedirs(output_dir, exist_ok=True)
    
    if not HAS_HARVESTERS:
        # Mock video stream
        mock_files = []
        for i in range(num_frames):
            time.sleep(0.1) # Simulate frame time
            frame_path = os.path.join(output_dir, f"frame_{i:04d}_0.jpg")
            jpeg_bytes = _generate_mock_jpeg(serial_number, i)
            with open(frame_path, "wb") as f:
                f.write(jpeg_bytes)
            mock_files.append(frame_path)
            logger.info(f"Mock acquired frame {i} saved to {frame_path}")
        return {"acquired_frames": mock_files}

    # Real mode
    conn = get_connection(cti_path, serial_number)
    ia = conn.acquirer
    try:
        ia.start()
        logger.info("Image acquisition started.")
        
        frame_files = []
        for i in range(num_frames):
            with ia.fetch() as buffer:
                payload = buffer.payload
                components = getattr(payload, 'components', [])
                
                # Support multi-component payload
                for c_idx, comp in enumerate(components):
                    img = convert_to_numpy_array(comp)
                    
                    if img is not None and HAS_OPENCV:
                        # Determine standard extension based on conversion success
                        frame_path = os.path.join(output_dir, f"frame_{i:04d}_{c_idx}.jpg")
                        cv2.imwrite(frame_path, img)
                    else:
                        frame_path = os.path.join(output_dir, f"frame_{i:04d}_{c_idx}.bin")
                        with open(frame_path, "wb") as f:
                            f.write(comp.data.tobytes())
                            
                    frame_files.append(frame_path)
                    logger.info(f"Acquired frame {i}, component {c_idx} saved to {frame_path}")
        
        ia.stop()
        logger.info("Image acquisition stopped.")
        return {"acquired_frames": frame_files}
    except Exception as e:
        logger.error(f"Error during video stream acquisition: {e}")
        try: ia.stop()
        except: pass
        raise e

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
    elif action == "close_all":
        close_all_connections()
        return {"status": "success"}
    elif action == "get_frame":
        # Direct retrieval of latest cached JPEG frame
        with _latest_frames_lock:
            frame_data = _latest_frames.get(serial_number, None)
        if frame_data:
            return {"jpeg_bytes": frame_data[0]}
        else:
            return {"jpeg_bytes": _generate_mock_jpeg(serial_number, 0)}
    else:
        raise ValueError(f"Unknown action: {action}")
