/*
 * Animation Kit — EffectCraft compatibility port
 * Self-contained build: Text Preset Volume I (1333 presets) is embedded below.
 * Preview note: original Flash/SWF player is replaced by a temporary preview comp.
 * Range-selector offsets are animated with native keyframes instead.
 */
var AK_PRESETS = [
    "2D^Range^Words,1,2,-100,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-50,0",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-75#Rotation X:50",
    "3D^Range^Characters,1,2,0,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:25#Rotation Y:-40#Position:0,120,0",
    "3D^Range^Characters,1,2,0,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-90#Anchor Point:0,0,50#Tracking Type:1#Tracking:-30",
    "2D^Range^Lines,1,2,-30,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:61#Skew:-60#Skew Axis:102",
    "2D^Range^Words,1,2,-84,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,240,0#Rotation:25",
    "3D^Range^Characters,1,2,-89,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:80#Position:-360,200,0",
    "3D^Range^Characters,1,2,-69,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:40#Position:360,-100,0",
    "2D^Range^Characters,1,2,-90,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Character Alignment:2#Character Range:2#Character Offset:-6",
    "2D^Range^Characters,1,2,-70,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:400,0,-300#Blur:10,30#Skew:-50#Skew Axis:72#Tracking Type:3#Tracking:-21",
    "2D^Range^Lines,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,-335,-35#Rotation:-148#Skew:2#Skew Axis:0#Tracking Type:1#Tracking:0",
    "2D^Range^Words,1,2,-97,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:0#Rotation:71",
    "2D^Range^Characters,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-148#Position:200,0,0",
    "2D^Range^Words,1,2,-88,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-73#Tracking Type:3#Tracking:-48#Position:0,-354,0#Skew:46#Skew Axis:78",
    "3D^Range^Characters,1,2,-70,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:0,-384,-364#Rotation Y:27#Anchor Point:0,0,0#Tracking Type:3#Tracking:0",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-130#Tracking Type:1#Tracking:-5#Skew:45#Skew Axis:3",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:173#Rotation X:86#Skew:69#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,84,11,100,-100!@ANIOM-PRESET@!Opacity:0#Position:104,0,0#Anchor Point:-460,489,-317",
    "3D^Range^Characters,1,2,-50,88,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:-94#Rotation X:0#Rotation:94#Skew:0#Skew Axis:0#Position:74,0,594",
    "2D^Range^Characters,1,2,-71,70,11,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,75,4,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:160#Anchor Point:-95,-281,0#Tracking Type:2#Tracking:2#Position:-463,523,0",
    "3D^Range^Lines,1,2,-73,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:63#Skew:54#Skew Axis:42#Anchor Point:-60,0,-45",
    "2D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:101#Skew:33#Skew Axis:43",
    "3D^Range^Words,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-502,0,-382#Rotation X:-175",
    "2D^Range^Characters,1,2,-75,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:-155#Position:102,0,0",
    "3D^Range^Characters,1,2,-50,92,11,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-163",
    "3D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Rotation:0#Rotation Y:-155",
    "3D^Range^Characters,1,2,-50,77,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:119#Tracking Type:2#Tracking:15#Rotation:-152#Skew:53#Skew Axis:69",
    "2D^Range^Words,1,2,-75,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-13#Anchor Point:94,409,0#Skew:0#Skew Axis:62#Position:-42,0,0",
    "3D^Range^Characters,1,2,-81,82,3,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:48#Skew Axis:0#Rotation X:47#Anchor Point:151,0,23",
    "2D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:31#Skew Axis:54#Rotation:-23#Anchor Point:-136,0,0#Tracking Type:2#Tracking:19#Position:-660,480,0",
    "2D^Range^Lines,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:-10#Tracking Type:3#Tracking:0",
    "2D^Range^Words,1,2,-76,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:12,17#Skew:10#Skew Axis:23#Anchor Point:0,50,-493#Tracking Type:3#Tracking:33",
    "3D^Range^Words,1,2,-82,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,19#Skew:16#Skew Axis:71#Position:-579,0,-339#Rotation X:-15",
    "3D^Range^Characters,1,2,-51,50,10,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:0#Position:-460,-39,0",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Skew:26#Skew Axis:0#Blur:0,0#Position:-20,0,-120",
    "2D^Range^Words,1,2,-89,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:549,0,-588#Anchor Point:349,0,377#Rotation:100#Skew:33#Skew Axis:42",
    "3D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:14#Rotation Y:115#Rotation X:0#Anchor Point:0,49,-335",
    "2D^Range^Characters,1,2,-48,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:7#Skew:47#Skew Axis:5#Anchor Point:91,-62,-434#Position:0,-127,528",
    "3D^Range^Lines,1,2,-94,83,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:58#Skew Axis:77#Anchor Point:-477,-163,-277#Position:0,-258,0#Rotation:0",
    "2D^Range^Words,1,2,-50,95,1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:98,0,78#Skew:23#Skew Axis:51#Anchor Point:-263,-63,0",
    "3D^Range^Characters,1,2,-58,50,11,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Tracking Type:3#Tracking:0#Position:0,567,0#Rotation Y:0#Rotation X:151",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Position:0,0,37#Rotation Y:0#Tracking Type:2#Tracking:0#Rotation X:-89",
    "2D^Range^Lines,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,17#Position:0,601,241#Skew:44#Skew Axis:9",
    "2D^Range^Characters,1,2,-80,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-611,531#Anchor Point:0,394,0",
    "2D^Range^Characters,1,2,-48,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Rotation:111#Position:291,0,331",
    "3D^Range^Characters,1,2,-50,71,5,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:22#Skew Axis:8#Rotation Y:159#Rotation X:-46",
    "2D^Range^Words,1,2,-93,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:21,6#Skew:12#Skew Axis:25#Tracking Type:2#Tracking:35#Anchor Point:-136,0,-250#Position:0,340,-560",
    "2D^Range^Characters,1,2,-51,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:50#Rotation:0#Position:0,469,0#Skew:27#Skew Axis:49",
    "2D^Range^Characters,1,2,-57,64,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:0,20#Position:0,330,-30#Tracking Type:3#Tracking:0#Skew:15#Skew Axis:0",
    "2D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Tracking Type:3#Tracking:35",
    "2D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:51#Blur:0,20#Rotation:0",
    "2D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:177,0,48#Rotation:-38#Skew:9#Skew Axis:69",
    "3D^Range^Characters,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:-28#Rotation Y:0#Tracking Type:1#Tracking:-50",
    "2D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-159#Tracking Type:1#Tracking:-31#Skew:52#Skew Axis:4",
    "2D^Range^Characters,1,2,-35,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:45#Position:0,0,586#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:0#Position:0,0,-267",
    "3D^Range^Characters,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:285,0,0#Tracking Type:2#Tracking:-16#Rotation Y:12#Skew:13#Skew Axis:17",
    "3D^Range^Lines,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:176,0,204#Tracking Type:3#Tracking:49#Rotation X:19",
    "2D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:22,0#Skew:0#Skew Axis:0#Anchor Point:360,0,0#Position:0,0,-384",
    "3D^Range^Characters,1,2,-46,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-1#Tracking Type:3#Tracking:35#Anchor Point:188,0,0#Rotation Y:171#Rotation X:-155#Skew:46#Skew Axis:30#Position:-404,604,644",
    "3D^Range^Characters,1,2,-96,98,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:14#Rotation:0#Rotation X:0#Skew:39#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,0#Skew:0#Skew Axis:0#Position:-323,0,-363",
    "2D^Range^Characters,1,2,-93,73,3,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:0#Position:493,0,0",
    "2D^Range^Characters,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:49#Anchor Point:-137,9,0",
    "3D^Range^Words,1,2,-62,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:0#Tracking Type:1#Tracking:-39#Skew:65#Skew Axis:33#Rotation:0",
    "3D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:21,6#Skew:11#Skew Axis:24#Tracking Type:1#Tracking:0#Rotation:119",
    "2D^Range^Characters,1,2,-89,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-261,-401,0#Anchor Point:-279,0,0#Skew:0#Skew Axis:65#Rotation:-136#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:264,50,207#Skew:0#Skew Axis:0#Position:0,0,-60",
    "3D^Range^Characters,1,2,-52,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:44#Skew Axis:16#Blur:20,10#Position:20,-400,0#Rotation Y:129#Tracking Type:2#Tracking:57#Rotation X:72",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:0,8#Position:79,0,599#Anchor Point:14,0,0",
    "2D^Range^Words,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:0#Position:-4,-524,0",
    "3D^Range^Lines,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:101#Skew:25#Skew Axis:61#Rotation X:-158#Rotation:14#Anchor Point:-95,66,438",
    "2D^Range^Characters,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-47#Blur:1,5#Position:503,0,0",
    "2D^Range^Characters,1,2,-48,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-101#Skew:0#Skew Axis:75#Tracking Type:2#Tracking:-20",
    "2D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:21#Skew Axis:49#Position:152,112,0#Blur:0,20#Tracking Type:3#Tracking:46#Rotation:36",
    "2D^Range^Characters,1,2,-78,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:57#Rotation:41#Anchor Point:-172,0,0#Blur:2,27",
    "2D^Range^Words,1,2,-40,50,3,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:165#Position:20,0,120#Blur:0,26",
    "2D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:0#Position:0,-649,0#Skew:9#Skew Axis:15",
    "2D^Range^Characters,1,2,-59,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-214,0",
    "3D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation Y:43#Tracking Type:3#Tracking:20#Skew:35#Skew Axis:0",
    "3D^Range^Lines,1,2,-50,74,14,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:12#Skew Axis:22#Position:0,-388,0#Rotation Y:7#Blur:15,0#Rotation X:-172#Rotation:-156",
    "2D^Range^Words,1,2,-85,88,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,8#Tracking Type:1#Tracking:-27#Skew:2#Skew Axis:24",
    "2D^Range^Words,1,2,-92,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,187,-467#Blur:0,0#Skew:10#Skew Axis:0#Rotation:0",
    "2D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:49#Skew Axis:7#Anchor Point:104,418,0#Blur:0,20#Tracking Type:3#Tracking:-46",
    "3D^Range^Words,1,2,-79,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,-404,347#Rotation X:81",
    "2D^Range^Lines,1,2,-94,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,0,0#Rotation:0#Skew:69#Skew Axis:38",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:55#Position:0,-584,-504",
    "3D^Range^Words,1,2,-74,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:1#Tracking:30",
    "3D^Range^Lines,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:5#Skew Axis:0#Rotation Y:80#Anchor Point:0,-37,0#Position:-192,472,-452#Rotation:0#Blur:16,0",
    "2D^Range^Characters,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:62#Skew Axis:43#Position:116,0,0#Tracking Type:2#Tracking:31",
    "2D^Range^Words,1,2,-77,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:587,546,-586#Blur:0,15#Skew:34#Skew Axis:44",
    "3D^Range^Words,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,0,476#Rotation:0#Rotation X:0#Skew:64#Skew Axis:0#Blur:23,0#Tracking Type:1#Tracking:8#Anchor Point:0,-368,197",
    "2D^Range^Words,1,2,-70,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-575,0,0#Tracking Type:2#Tracking:0#Rotation:0",
    "2D^Range^Characters,1,2,-80,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,210,196",
    "3D^Range^Lines,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:20,5#Skew:9#Skew Axis:22#Rotation X:-73#Anchor Point:0,-217,0#Tracking Type:2#Tracking:-55",
    "3D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,0,573#Tracking Type:1#Tracking:0#Rotation Y:-155#Blur:10,10#Skew:0#Skew Axis:53#Anchor Point:0,-59,0",
    "3D^Range^Characters,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,0,-401#Rotation X:11#Position:0,362,-62#Rotation Y:0#Blur:16,0",
    "2D^Range^Characters,1,2,-91,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-55#Rotation:-67#Skew:59#Skew Axis:65",
    "2D^Range^Lines,1,2,-83,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Skew:39#Skew Axis:0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:32#Skew Axis:55#Position:0,0,-400#Rotation:10#Tracking Type:1#Tracking:47",
    "3D^Range^Characters,1,2,-96,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:269,-149,0#Rotation X:28#Rotation:154#Skew:54#Skew Axis:80",
    "2D^Range^Characters,1,2,-50,96,4,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:568,0,0#Anchor Point:463,-334,0",
    "3D^Range^Words,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:11#Skew Axis:24#Rotation X:-76",
    "2D^Range^Lines,1,2,-71,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:12#Skew Axis:0#Rotation:15#Anchor Point:-97,-412,440",
    "2D^Range^Characters,1,2,-50,99,8,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:54#Skew Axis:74#Tracking Type:2#Tracking:-27#Rotation:24",
    "2D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:54#Skew Axis:48#Blur:26,0",
    "3D^Range^Words,1,2,-89,98,5,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:-27#Rotation X:0#Position:0,0,0",
    "3D^Range^Lines,1,2,-50,95,4,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:9#Skew Axis:28#Rotation:0#Anchor Point:0,-62,0",
    "2D^Range^Characters,1,2,-36,50,8,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Skew:0#Skew Axis:38",
    "2D^Range^Words,1,2,-99,94,12,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:46#Skew Axis:68#Rotation:30#Blur:22,5",
    "3D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:155#Tracking Type:3#Tracking:0#Rotation Y:0#Blur:20,5",
    "3D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:29#Blur:0,6#Position:0,12,112#Skew:9#Skew Axis:22#Rotation X:73#Anchor Point:0,216,0#Tracking Type:2#Tracking:55",
    "2D^Range^Words,1,2,-81,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,93,10,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:24#Skew Axis:52#Anchor Point:272,72,443#Position:-521,0,0#Blur:23,21#Tracking Type:3#Tracking:3",
    "2D^Range^Lines,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:21,26#Skew:11#Skew Axis:24#Tracking Type:2#Tracking:-15#Rotation:-90",
    "3D^Range^Characters,1,2,-35,50,8,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-28#Rotation Y:103#Blur:20,12",
    "3D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-478,0,-518#Rotation:107#Skew:0#Skew Axis:77#Rotation X:87#Rotation Y:51",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Tracking Type:3#Tracking:0#Rotation Y:0#Anchor Point:0,226,0#Blur:0,28",
    "3D^Range^Characters,1,2,-50,95,5,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-356,-57#Rotation:135#Blur:1,5#Rotation X:-53",
    "3D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Tracking Type:3#Tracking:-23#Rotation Y:0#Rotation X:0#Position:191,470,450",
    "2D^Range^Lines,1,2,-50,68,8,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:9#Skew Axis:22#Tracking Type:2#Tracking:0",
    "2D^Range^Words,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:169#Position:368,0,0",
    "3D^Range^Lines,1,2,-40,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-597,0#Blur:0,21#Tracking Type:1#Tracking:55",
    "2D^Range^Characters,1,2,-87,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:61#Skew Axis:37#Position:106,0,85#Anchor Point:0,-375,0",
    "3D^Range^Characters,1,2,-93,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:163#Anchor Point:10,68,0#Tracking Type:3#Tracking:24#Rotation Y:32",
    "3D^Range^Characters,1,2,-46,52,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Position:44,0,0#Skew:24#Skew Axis:67#Tracking Type:3#Tracking:45#Rotation Y:-130",
    "3D^Range^Characters,1,2,-50,71,6,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:22#Skew Axis:8#Rotation Y:160#Anchor Point:0,74,474#Position:0,644,0",
    "2D^Range^Words,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:42,0,-142#Skew:32#Skew Axis:42#Blur:27,0",
    "3D^Range^Lines,1,2,-64,60,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:31#Blur:17,10#Rotation Y:-167#Tracking Type:2#Tracking:-20",
    "2D^Range^Characters,1,2,-50,85,6,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:35#Rotation:142#Skew:21#Skew Axis:49#Position:-150,110,0#Blur:17,0",
    "3D^Range^Lines,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:74#Tracking Type:1#Tracking:20#Skew:55#Skew Axis:1#Rotation:-84#Anchor Point:306,0,420",
    "2D^Range^Words,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-93,0,0#Blur:26,0#Skew:0#Skew Axis:13#Rotation:0",
    "3D^Range^Characters,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:20,5#Skew:0#Skew Axis:0#Rotation Y:0#Tracking Type:3#Tracking:35",
    "2D^Range^Words,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:0#Blur:21,26#Skew:11#Skew Axis:24",
    "2D^Range^Words,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:7#Skew Axis:19#Blur:16,6#Tracking Type:1#Tracking:-39",
    "2D^Range^Words,1,2,-89,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Rotation:144#Skew:22#Skew Axis:57",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Blur:26,0#Skew:0#Skew Axis:0#Position:0,0,618#Rotation:0",
    "2D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-71#Anchor Point:-98,-69,0#Blur:0,19#Tracking Type:3#Tracking:45#Skew:11#Skew Axis:24",
    "2D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-455,475,635#Anchor Point:0,0,0",
    "2D^Range^Characters,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-31",
    "2D^Range^Words,1,2,-98,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:7#Anchor Point:0,-359,0#Skew:49#Skew Axis:61#Rotation:-101",
    "3D^Range^Characters,1,2,-80,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:73#Rotation Y:32#Position:0,303,0",
    "3D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:48#Rotation Y:-152",
    "3D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:24,0#Rotation:143#Skew:60#Skew Axis:12#Anchor Point:182,-68,-268#Tracking Type:2#Tracking:17",
    "3D^Range^Characters,1,2,-50,77,11,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-59#Rotation X:24#Position:0,-334,0#Rotation Y:-176#Anchor Point:-453,0,-482",
    "2D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,652,0",
    "2D^Range^Words,1,2,-86,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,29#Skew:0#Skew Axis:64",
    "3D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-57#Skew:14#Skew Axis:69#Position:-560,0,0#Rotation Y:62#Anchor Point:-457,329,0#Rotation X:154",
    "3D^Range^Characters,1,2,-50,76,10,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0#Tracking Type:1#Tracking:0#Rotation:0",
    "2D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:420,0,0#Blur:0,0#Rotation:100",
    "3D^Range^Lines,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-105#Tracking Type:3#Tracking:19#Position:-307,0,0#Skew:1#Skew Axis:33",
    "3D^Range^Characters,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:0#Position:-165,-125,-645#Anchor Point:0,0,0#Skew:58#Skew Axis:0#Blur:22,0#Rotation:0",
    "3D^Range^Characters,1,2,-50,87,10,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-38#Blur:0,20#Anchor Point:0,-200,315",
    "2D^Range^Words,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:-108,0,0#Skew:25#Skew Axis:0#Rotation:179",
    "2D^Range^Lines,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-49#Position:-66,0,166#Blur:6,0#Skew:55#Skew Axis:74",
    "2D^Range^Characters,1,2,-38,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-169#Position:-666,0,0",
    "3D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:39#Rotation:0#Tracking Type:1#Tracking:0#Rotation Y:83#Anchor Point:0,-203,0#Position:424,-484,-464",
    "3D^Range^Characters,1,2,-84,93,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:0#Rotation Y:0#Blur:16,0",
    "3D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:106#Position:-62,62,0#Blur:0,6#Skew:54#Skew Axis:64#Tracking Type:1#Tracking:-15",
    "3D^Range^Characters,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:20#Position:-310,0,0#Skew:1#Skew Axis:33#Rotation X:39#Rotation:131#Anchor Point:-264,64,0#Rotation Y:-36",
    "2D^Range^Characters,1,2,-50,89,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-28#Skew:5#Skew Axis:21#Position:-468,-368,68",
    "2D^Range^Characters,1,2,-84,92,6,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:62#Skew Axis:44#Position:-342,0,0",
    "2D^Range^Characters,1,2,-66,58,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:54#Skew Axis:48#Blur:27,0#Anchor Point:0,-336,0",
    "3D^Range^Lines,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,350,0#Rotation X:0#Anchor Point:0,186,0",
    "2D^Range^Characters,1,2,-82,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:12#Skew Axis:25#Rotation:61#Blur:0,29#Tracking Type:1#Tracking:-8",
    "3D^Range^Words,1,2,-36,50,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:0#Skew:64#Skew Axis:0#Blur:23,0#Tracking Type:2#Tracking:8#Anchor Point:0,-369,198#Rotation:0#Rotation Y:-12",
    "3D^Range^Words,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:33#Skew Axis:49#Rotation:27",
    "3D^Range^Characters,1,2,-50,84,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation Y:0#Skew:24#Skew Axis:0#Tracking Type:1#Tracking:-7#Rotation:0#Position:695,-515,0",
    "3D^Range^Lines,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:0#Skew Axis:0#Rotation Y:0#Rotation X:0#Position:-194,-473,-453#Rotation:50#Tracking Type:1#Tracking:0",
    "3D^Range^Characters,1,2,-50,85,2,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:63#Skew Axis:45#Rotation Y:-29#Position:552,-11,-231#Rotation X:-106",
    "2D^Range^Words,1,2,-89,77,10,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:65#Anchor Point:0,-32,0#Rotation:0",
    "3D^Range^Characters,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:63#Rotation X:112#Blur:20,0#Tracking Type:3#Tracking:47#Rotation Y:11#Skew:12#Skew Axis:26#Anchor Point:24,0,-310",
    "3D^Range^Characters,1,2,-98,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:15#Skew Axis:63#Position:0,-93,-493#Rotation X:-52#Tracking Type:1#Tracking:-31",
    "3D^Range^Words,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:22#Rotation Y:0#Anchor Point:0,73,0#Rotation:0",
    "3D^Range^Words,1,2,-50,66,5,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Position:0,350,0#Rotation Y:-149#Rotation:82",
    "3D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:159#Position:-119,0,98#Tracking Type:2#Tracking:-33#Anchor Point:-177,0,0#Rotation Y:-167",
    "3D^Range^Characters,1,2,-93,95,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:61#Blur:0,29#Tracking Type:1#Tracking:29#Skew:52#Skew Axis:0#Position:0,537,-297",
    "3D^Range^Words,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-102#Rotation X:-122#Skew:8#Skew Axis:18#Blur:7,0#Rotation Y:138",
    "3D^Range^Characters,1,2,-45,50,5,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-49#Blur:23,0#Skew:49#Skew Axis:2#Rotation Y:-50#Rotation X:39",
    "2D^Range^Words,1,2,-74,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-678,498,0#Rotation:-30#Blur:22,5#Tracking Type:3#Tracking:-38",
    "3D^Range^Words,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:297,0,0#Rotation X:0#Rotation:-61#Blur:0,29",
    "3D^Range^Characters,1,2,-77,86,2,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-27#Rotation:158#Anchor Point:-496,0,0#Skew:20#Skew Axis:6#Rotation Y:-153",
    "2D^Range^Characters,1,2,-50,66,12,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:16#Skew Axis:64#Position:0,103,-503#Rotation:-55#Blur:0,28#Tracking Type:2#Tracking:0",
    "2D^Range^Words,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-83#Skew:46#Skew Axis:30#Anchor Point:87,-58,0",
    "3D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-87#Position:0,360,0#Rotation X:152#Rotation:-85#Tracking Type:2#Tracking:8",
    "2D^Range^Words,1,2,-98,78,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-90#Tracking Type:1#Tracking:0#Skew:16#Skew Axis:50",
    "2D^Range^Characters,1,2,-65,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-164#Anchor Point:12,0,0#Blur:22,27",
    "2D^Range^Characters,1,2,-43,50,5,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:37#Skew Axis:47#Position:310,0,349#Anchor Point:-178,0,0#Rotation:-131",
    "3D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:74#Anchor Point:-106,-77,106#Rotation X:2#Skew:57#Skew Axis:61#Rotation Y:141",
    "2D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0",
    "2D^Range^Characters,1,2,-59,74,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:43#Rotation:-104#Anchor Point:-189,0,0",
    "3D^Range^Characters,1,2,-84,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:0#Rotation Y:-106#Blur:0,0",
    "3D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-78#Position:-162,122,0#Skew:0#Skew Axis:38",
    "2D^Range^Characters,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-490,0,-590#Skew:7#Skew Axis:17#Blur:6,0",
    "3D^Range^Characters,1,2,-56,39,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Rotation:0#Position:0,309,529#Tracking Type:2#Tracking:8#Anchor Point:0,-363,-349",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:12#Skew Axis:16#Position:152,-612,0#Rotation X:-60#Tracking Type:2#Tracking:-44#Rotation Y:-178",
    "3D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:0,10#Rotation X:0#Rotation:0#Tracking Type:3#Tracking:0",
    "3D^Range^Lines,1,2,-50,96,3,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:50#Skew:20#Skew Axis:68#Blur:9,10#Rotation:87",
    "2D^Range^Characters,1,2,-50,70,8,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Tracking Type:2#Tracking:0#Position:0,386,606",
    "2D^Range^Lines,1,2,-50,71,14,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:289,332,0#Rotation:-27#Blur:0,7#Tracking Type:1#Tracking:30",
    "2D^Range^Characters,1,2,-35,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:52#Skew Axis:4#Position:-663,483,0#Rotation:150",
    "2D^Range^Lines,1,2,-50,67,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:512,0,551#Rotation:-106#Skew:35#Skew Axis:45",
    "2D^Range^Characters,1,2,-50,68,13,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:4,0#Skew:0#Skew Axis:0#Anchor Point:0,7,0#Position:0,410,-210",
    "3D^Range^Characters,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Skew:0#Skew Axis:48#Anchor Point:489,0,103#Position:594,0,0#Tracking Type:2#Tracking:0#Rotation X:88",
    "2D^Range^Characters,1,2,-50,69,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Rotation:-140#Position:623,-123,-162#Skew:62#Skew Axis:76",
    "3D^Range^Characters,1,2,-86,81,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:0#Rotation:-152#Position:90,0,-70#Anchor Point:0,0,-478#Rotation X:0#Blur:0,29#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:23",
    "3D^Range^Lines,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-64#Tracking Type:2#Tracking:-28#Skew:53#Skew Axis:63#Blur:5,6#Anchor Point:0,234,0#Rotation X:-77",
    "3D^Range^Words,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,377,77#Tracking Type:2#Tracking:30#Blur:4,0",
    "2D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:6,0#Skew:55#Skew Axis:74#Rotation:0",
    "3D^Range^Characters,1,2,-98,83,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:39#Rotation X:-140#Blur:0,0",
    "2D^Range^Words,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:36#Skew Axis:45#Position:296,0,-336#Tracking Type:1#Tracking:-33",
    "3D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:-482,0,0#Skew:43#Skew Axis:41#Blur:29,10#Tracking Type:2#Tracking:60#Rotation:-60#Position:565,-24,-244#Rotation Y:-176",
    "3D^Range^Characters,1,2,-92,81,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:503,0,0#Tracking Type:1#Tracking:0#Rotation:168",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-659,699,-459#Blur:0,9#Tracking Type:1#Tracking:-25#Skew:37#Skew Axis:41",
    "3D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-172#Position:48,0,0",
    "2D^Range^Characters,1,2,-50,84,2,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-156#Skew:55#Skew Axis:1#Position:-487,-386,0#Anchor Point:-305,0,-419",
    "3D^Range^Characters,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:-174#Rotation:14#Rotation X:0#Blur:0,7#Position:-485,0,365",
    "3D^Range^Lines,1,2,-41,55,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:23,21#Tracking Type:1#Tracking:6#Rotation X:101#Rotation:-86",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:-34#Tracking Type:3#Tracking:-16#Rotation Y:0#Skew:21#Skew Axis:0",
    "2D^Range^Characters,1,2,-36,65,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:66#Rotation:82#Tracking Type:2#Tracking:-27#Anchor Point:0,42,0",
    "2D^Range^Words,1,2,-50,96,10,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:151#Skew:53#Skew Axis:79",
    "3D^Range^Words,1,2,-93,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-60#Blur:2,26#Rotation X:-56#Position:297,0,-397",
    "3D^Range^Characters,1,2,-71,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Blur:10,10#Skew:56#Skew Axis:0#Anchor Point:0,60,346#Tracking Type:2#Tracking:-29",
    "3D^Range^Lines,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:7,87,-367#Skew:70#Skew Axis:39#Rotation X:138",
    "2D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:50#Skew Axis:2#Anchor Point:0,0,0",
    "3D^Range^Words,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:104#Tracking Type:2#Tracking:-19#Position:303,0,-123#Rotation:0#Anchor Point:-102,-74,0#Skew:50#Skew Axis:10",
    "2D^Range^Characters,1,2,-41,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:3#Skew Axis:54#Tracking Type:3#Tracking:-49",
    "2D^Range^Words,1,2,-65,78,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-82#Position:0,-179,-459#Blur:0,0#Anchor Point:464,0,0#Skew:0#Skew Axis:38",
    "3D^Range^Characters,1,2,-50,66,9,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:0",
    "3D^Range^Lines,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:138#Position:397,117,637#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0#Rotation Y:0",
    "3D^Range^Characters,1,2,-50,82,0,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Skew:0#Skew Axis:0#Position:179,-700,460#Blur:0,9#Tracking Type:2#Tracking:0#Rotation:0",
    "3D^Range^Lines,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Rotation Y:179#Position:-437,-17,477#Rotation:-56#Blur:0,28",
    "2D^Range^Characters,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,425,0#Tracking Type:2#Tracking:38#Skew:0#Skew Axis:0",
    "2D^Range^Lines,1,2,-82,83,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:0,-366,195#Skew:68#Skew Axis:30#Position:622,122,0#Rotation:16#Blur:0,0",
    "2D^Range^Characters,1,2,-91,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-581,501#Anchor Point:0,208,193#Rotation:57",
    "3D^Range^Characters,1,2,-50,82,8,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:33#Skew Axis:21#Rotation Y:-149",
    "2D^Range^Characters,1,2,-78,84,3,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:16#Rotation:-155",
    "2D^Range^Lines,1,2,-46,84,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:1#Tracking:22#Skew:34#Skew Axis:18#Rotation:-46",
    "3D^Range^Characters,1,2,-45,50,6,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Position:0,-608,0#Anchor Point:0,-270,0#Rotation Y:-66#Blur:25,0",
    "2D^Range^Words,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,30",
    "3D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation X:55#Rotation Y:0#Position:0,654,-14#Skew:0#Skew Axis:35",
    "2D^Range^Lines,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:533,-153,0#Anchor Point:-181,66,-266#Tracking Type:3#Tracking:17#Rotation:44",
    "2D^Range^Characters,1,2,-87,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:64#Skew Axis:39#Position:0,-364,0#Tracking Type:2#Tracking:-28#Rotation:-140",
    "3D^Range^Words,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,0#Rotation X:0#Skew:0#Skew Axis:10#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-65,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-172,0,0#Skew:22#Skew Axis:50#Position:-161,-120,-641",
    "3D^Range^Characters,1,2,-69,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:61#Skew Axis:0#Tracking Type:2#Tracking:0#Position:0,0,-138#Rotation X:-92#Rotation Y:0",
    "2D^Range^Characters,1,2,-92,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-27#Blur:23,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0",
    "2D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:9#Position:-544,0,-524#Skew:3#Skew Axis:0",
    "2D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:21,26#Skew:11#Skew Axis:24#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-84,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-143#Rotation Y:-163#Skew:50#Skew Axis:8#Position:155,-115,0#Blur:17,21",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,0#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-71,76,11,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:0#Rotation X:-10",
    "3D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-43",
    "3D^Range^Words,1,2,-87,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Skew:67#Skew Axis:67#Position:0,607,-527#Anchor Point:0,-391,0",
    "3D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Tracking Type:2#Tracking:0#Rotation X:-98",
    "2D^Range^Words,1,2,-69,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:12#Skew Axis:0#Rotation:0#Position:0,147,-247",
    "3D^Range^Characters,1,2,-97,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,-626,0#Rotation Y:0#Tracking Type:3#Tracking:0#Rotation:9#Blur:15,0",
    "2D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-564,0,543#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-77,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:44#Skew:11#Skew Axis:72#Position:531,0,0",
    "3D^Range^Words,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Rotation X:0#Position:-39,0,-139",
    "3D^Range^Words,1,2,-49,64,8,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:22#Blur:22,0#Rotation:-5#Tracking Type:2#Tracking:29",
    "2D^Range^Characters,1,2,-50,68,11,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-144#Skew:30#Skew Axis:59#Tracking Type:2#Tracking:7",
    "3D^Range^Characters,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:2,6#Rotation X:56#Position:0,-247,0",
    "2D^Range^Characters,1,2,-50,72,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:0,-211,-196#Rotation:-120",
    "2D^Range^Characters,1,2,-93,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-75#Tracking Type:2#Tracking:35#Skew:13#Skew Axis:13#Position:552,0,0",
    "3D^Range^Words,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:19#Skew Axis:61#Rotation:8#Tracking Type:2#Tracking:-31#Blur:20,10#Position:21,-401,321",
    "2D^Range^Characters,1,2,-57,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-461,333,318",
    "3D^Range^Characters,1,2,-35,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:0,-402,-145#Position:0,-403,0#Rotation X:0#Rotation:-109",
    "2D^Range^Lines,1,2,-48,59,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,0,0#Rotation:0#Tracking Type:3#Tracking:0#Blur:0,28#Skew:0#Skew Axis:62",
    "3D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-128#Rotation X:180",
    "3D^Range^Words,1,2,-94,72,3,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:38#Skew Axis:54#Rotation X:0#Rotation Y:115",
    "2D^Range^Characters,1,2,-50,98,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:64#Tracking Type:2#Tracking:0#Position:-700,520,500#Blur:0,29",
    "2D^Range^Characters,1,2,-53,99,2,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-57",
    "3D^Range^Characters,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:84#Position:0,0,405#Rotation X:0#Anchor Point:489,0,0",
    "3D^Range^Lines,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0",
    "2D^Range^Words,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:-6#Anchor Point:0,364,0",
    "3D^Range^Words,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:25#Rotation:151",
    "3D^Range^Words,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:0",
    "3D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:-169#Rotation X:103#Rotation:-126#Anchor Point:178,0,0#Tracking Type:2#Tracking:16",
    "3D^Range^Characters,1,2,-39,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:0#Rotation X:0#Blur:11,6",
    "3D^Range^Characters,1,2,-54,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:40#Rotation:0#Tracking Type:3#Tracking:0#Position:0,294,0",
    "3D^Range^Words,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-152#Anchor Point:0,-415,0#Rotation:0#Skew:0#Skew Axis:0#Tracking Type:2#Tracking:-22",
    "2D^Range^Characters,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:20,10#Position:-24,-404,0#Tracking Type:1#Tracking:55#Skew:0#Skew Axis:6",
    "2D^Range^Lines,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:6,0#Position:0,0,0#Skew:0#Skew Axis:36#Rotation:75",
    "3D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Rotation:123#Skew:0#Skew Axis:67#Blur:0,0#Position:159,119,0#Tracking Type:3#Tracking:0",
    "2D^Range^Words,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:26#Rotation:-156#Anchor Point:0,341,-283",
    "2D^Range^Characters,1,2,-50,69,4,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:489,0,-60#Rotation:42#Tracking Type:2#Tracking:0#Position:-45,0,145",
    "2D^Range^Characters,1,2,-89,83,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-564,664#Blur:16,0#Skew:34#Skew Axis:0#Rotation:0",
    "2D^Range^Characters,1,2,-51,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:61#Skew Axis:0#Tracking Type:3#Tracking:0#Position:-644,0,-464#Blur:0,17",
    "2D^Range^Characters,1,2,-50,95,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:173,0,0#Skew:22#Skew Axis:57#Position:162,-121,-642",
    "2D^Range^Characters,1,2,-50,84,13,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,364,349#Tracking Type:3#Tracking:15#Blur:0,7",
    "3D^Range^Characters,1,2,-83,92,5,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:159#Blur:0,10#Rotation X:0#Rotation:92",
    "2D^Range^Words,1,2,-40,43,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:-465,-525,505#Anchor Point:0,-418,-160#Skew:42#Skew Axis:7",
    "2D^Range^Words,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:22#Skew Axis:0#Position:0,-407,507#Anchor Point:5,0,-76",
    "2D^Range^Words,1,2,-87,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:48#Position:-45,0,0#Skew:24#Skew Axis:12",
    "3D^Range^Words,1,2,-38,35,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:122#Rotation Y:-24#Position:0,113,513",
    "2D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:0#Anchor Point:458,-329,0",
    "2D^Range^Characters,1,2,-50,68,2,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:10#Skew:24#Skew Axis:66#Blur:11,26",
    "2D^Range^Words,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,341,0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-92#Skew:0#Skew Axis:50#Blur:29,10#Tracking Type:1#Tracking:60#Anchor Point:0,228,0",
    "2D^Range^Words,1,2,-50,94,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:8#Skew Axis:27#Position:17,157,-77#Tracking Type:1#Tracking:52#Blur:26,0",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-83#Anchor Point:0,-45,31",
    "2D^Range^Words,1,2,-74,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:286,-329,-315#Skew:0#Skew Axis:66#Rotation:-83#Tracking Type:2#Tracking:27",
    "2D^Range^Lines,1,2,-81,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0#Tracking Type:3#Tracking:0#Blur:0,0#Anchor Point:-2,0,74#Rotation:-47",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:126#Rotation:-101#Skew:33#Skew Axis:37",
    "3D^Range^Characters,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Tracking Type:2#Tracking:-22#Rotation Y:180#Position:-659,479,0#Rotation X:92#Blur:0,0",
    "2D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-18#Skew:31#Skew Axis:53#Rotation:22#Position:-507,407,0#Anchor Point:-412,0,0",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-103,0#Skew:30#Skew Axis:53#Tracking Type:1#Tracking:-40#Rotation X:-21#Anchor Point:0,231,17#Rotation:96",
    "3D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:157#Tracking Type:2#Tracking:16#Rotation Y:13#Skew:13#Skew Axis:20#Position:72,0,592",
    "2D^Range^Characters,1,2,-89,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:60#Skew Axis:36#Position:320,0,0",
    "3D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:146#Position:0,-107,0#Skew:31#Skew Axis:53#Tracking Type:1#Tracking:-59#Rotation X:23",
    "3D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-52#Anchor Point:360,0,0",
    "2D^Range^Words,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:62#Tracking Type:1#Tracking:-10#Blur:24,0#Skew:3#Skew Axis:25",
    "3D^Range^Characters,1,2,-88,88,14,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:29,10#Rotation Y:6#Tracking Type:2#Tracking:55#Position:-594,0,0#Skew:0#Skew Axis:0",
    "3D^Range^Words,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,531,0#Rotation Y:-54#Rotation X:-44#Skew:68#Skew Axis:2",
    "2D^Range^Lines,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,9#Tracking Type:1#Tracking:5#Rotation:0#Skew:0#Skew Axis:48#Position:0,-359,-59",
    "2D^Range^Characters,1,2,-46,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:8#Skew Axis:27#Rotation:86",
    "3D^Range^Characters,1,2,-83,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-583,43,263#Tracking Type:2#Tracking:24#Rotation X:-6#Rotation:-124",
    "3D^Range^Lines,1,2,-63,95,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:-76#Tracking Type:3#Tracking:0#Rotation Y:0",
    "3D^Range^Words,1,2,-78,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:147#Rotation X:19#Skew:54#Skew Axis:0",
    "2D^Range^Lines,1,2,-41,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-229,0#Blur:8,11#Rotation:167#Skew:37#Skew Axis:13",
    "2D^Range^Characters,1,2,-65,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:2#Skew Axis:30#Position:0,-439,0",
    "3D^Range^Characters,1,2,-40,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Tracking Type:2#Tracking:13#Blur:0,0#Rotation X:98#Skew:0#Skew Axis:23",
    "3D^Range^Characters,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:-169#Tracking Type:2#Tracking:-20#Rotation:0",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:43#Skew Axis:35#Blur:29,10#Tracking Type:1#Tracking:59#Anchor Point:0,224,10#Rotation:14",
    "3D^Range^Characters,1,2,-37,82,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:0#Blur:0,0#Position:22,0,0#Skew:22#Skew Axis:9#Rotation Y:160#Anchor Point:0,73,0",
    "2D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-420,700,-460",
    "3D^Range^Characters,1,2,-50,71,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:45#Rotation:0#Anchor Point:0,407,150#Skew:41#Skew Axis:6",
    "2D^Range^Lines,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:5#Skew Axis:31#Position:-188,8,0",
    "3D^Range^Lines,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-127#Tracking Type:3#Tracking:46#Position:-565,-325,-665#Rotation:6#Skew:0#Skew Axis:65",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Skew:33#Skew Axis:0#Tracking Type:2#Tracking:20",
    "3D^Range^Characters,1,2,-65,47,6,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Rotation X:0#Position:0,-574,0#Skew:64#Skew Axis:46#Tracking Type:2#Tracking:0#Rotation:14",
    "2D^Range^Characters,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,26#Skew:10#Skew Axis:23#Anchor Point:0,-223,0",
    "2D^Range^Characters,1,2,-50,78,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:15,0#Tracking Type:2#Tracking:0#Skew:4#Skew Axis:0",
    "2D^Range^Words,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-138#Position:677,17,0",
    "2D^Range^Characters,1,2,-50,70,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:24#Skew Axis:0",
    "2D^Range^Words,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:687,-27,0#Rotation:89#Blur:4,0#Tracking Type:2#Tracking:9",
    "3D^Range^Characters,1,2,-87,78,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:6#Skew:10#Skew Axis:23#Tracking Type:3#Tracking:25",
    "2D^Range^Characters,1,2,-50,65,11,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:3#Skew Axis:22#Tracking Type:1#Tracking:56#Anchor Point:0,272,0",
    "2D^Range^Characters,1,2,-98,73,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:37",
    "3D^Range^Words,1,2,-84,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:-157#Blur:27,0#Skew:0#Skew Axis:14#Rotation X:-82#Rotation:134",
    "2D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Position:438,0,0#Blur:0,11",
    "2D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:27#Skew:4#Skew Axis:20#Tracking Type:1#Tracking:-57#Blur:29,10#Anchor Point:-96,-139,0",
    "2D^Range^Characters,1,2,-82,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-162,-122,0#Rotation:-134#Anchor Point:-273,-416,0#Blur:12,0",
    "3D^Range^Words,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:21,0#Rotation Y:-59#Position:188,-9,469#Rotation:0",
    "2D^Range^Characters,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,-352,-53#Tracking Type:3#Tracking:35#Blur:7,0",
    "3D^Range^Lines,1,2,-83,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-80,541#Anchor Point:0,357,0#Rotation X:-46#Tracking Type:2#Tracking:50#Rotation Y:108#Blur:21,26#Skew:10#Skew Axis:20",
    "3D^Range^Lines,1,2,-60,50,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,-167#Rotation:0#Skew:29#Skew Axis:51#Tracking Type:1#Tracking:0#Anchor Point:0,0,0",
    "2D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:19#Position:435,15,0",
    "3D^Range^Words,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:0#Tracking Type:2#Tracking:8#Anchor Point:0,0,483#Rotation X:0#Blur:0,29",
    "3D^Range^Words,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,28#Tracking Type:1#Tracking:0#Rotation Y:99#Rotation:11#Rotation X:0",
    "2D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,0,0#Anchor Point:0,387,-329#Blur:0,21#Rotation:5#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-88,97,2,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:0#Rotation Y:0#Anchor Point:0,127,-327#Rotation:0",
    "2D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-130#Blur:17,1#Tracking Type:3#Tracking:-17",
    "2D^Range^Words,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:60#Skew Axis:41#Position:97,0,77#Anchor Point:-369,484,-141#Tracking Type:3#Tracking:-48#Rotation:-15",
    "3D^Range^Words,1,2,-57,50,4,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:61#Rotation Y:173#Rotation X:0#Blur:0,19",
    "2D^Range^Words,1,2,-55,50,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Blur:0,8#Anchor Point:186,0,0#Position:0,-441,-241",
    "2D^Range^Words,1,2,-95,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Rotation:0#Skew:60#Skew Axis:0#Blur:0,12",
    "2D^Range^Characters,1,2,-50,73,14,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,591,0#Anchor Point:-300,0,-415#Blur:15,0#Rotation:-85",
    "2D^Range^Characters,1,2,-36,41,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:558,0,-598#Anchor Point:355,0,384#Tracking Type:2#Tracking:-6#Rotation:0",
    "2D^Range^Characters,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Position:0,-114,0#Skew:31#Skew Axis:54#Tracking Type:1#Tracking:0",
    "3D^Range^Words,1,2,-99,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:611,111,0#Rotation:13#Anchor Point:-93,-408,-436#Skew:8#Skew Axis:66#Tracking Type:2#Tracking:-48",
    "3D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Position:-352,0,-332#Rotation X:0",
    "3D^Range^Lines,1,2,-73,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-80,580,-40#Rotation:-93#Skew:30#Skew Axis:46#Tracking Type:1#Tracking:58#Rotation X:-77",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Rotation Y:0#Rotation X:-143#Position:0,-577,497",
    "2D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:-64,0,0#Rotation:68#Tracking Type:3#Tracking:29#Skew:54#Skew Axis:74",
    "3D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:132,332,318#Skew:18#Skew Axis:19#Tracking Type:2#Tracking:36#Position:-315,0,0",
    "3D^Range^Words,1,2,-50,93,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-35#Position:482,0,0#Rotation X:-160#Rotation:93#Blur:4,0",
    "3D^Range^Lines,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,0#Rotation X:-95#Tracking Type:2#Tracking:16#Anchor Point:-192,0,-63#Rotation:0",
    "2D^Range^Lines,1,2,-90,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,212,198#Blur:2,27",
    "2D^Range^Characters,1,2,-56,71,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:162#Anchor Point:-6,0,78#Position:289,0,0",
    "2D^Range^Words,1,2,-91,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:1#Tracking:-6#Anchor Point:0,361,347#Rotation:0",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:46#Skew Axis:10#Rotation:-152",
    "2D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:0,-371,-357#Position:-690,0,0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-77,72,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:0,222,0#Blur:8,0#Rotation:0#Tracking Type:3#Tracking:0",
    "2D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:127,284,27#Tracking Type:3#Tracking:52#Rotation:-59",
    "2D^Range^Characters,1,2,-74,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-195,-255,0#Anchor Point:-4,-61,0#Tracking Type:1#Tracking:-13#Skew:69#Skew Axis:34",
    "2D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-47#Position:-32,0,132#Skew:31#Skew Axis:47",
    "3D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-132#Anchor Point:424,-67,0#Skew:3#Skew Axis:36#Position:684,564,-664#Rotation Y:50#Blur:9,10",
    "3D^Range^Characters,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,20#Rotation X:-23#Position:0,-331,31#Skew:36#Skew Axis:14#Rotation Y:21#Anchor Point:0,-372,0",
    "3D^Range^Characters,1,2,-86,76,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:2,6#Rotation X:56#Position:-517,0,0",
    "2D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:3#Tracking:0#Rotation:-73",
    "3D^Range^Characters,1,2,-50,91,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-22#Blur:23,0#Tracking Type:1#Tracking:-8#Anchor Point:0,368,-197#Position:0,-426,0",
    "2D^Range^Words,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:0#Skew Axis:38#Rotation:0#Tracking Type:1#Tracking:0#Anchor Point:0,-195,0#Position:413,-473,-453",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:154#Position:0,0,0#Blur:9,10#Skew:0#Skew Axis:0#Rotation:87",
    "2D^Range^Lines,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:7,22#Anchor Point:-4,0,0#Skew:0#Skew Axis:0#Position:0,-406,46",
    "3D^Range^Characters,1,2,-73,97,15,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,378,0#Anchor Point:327,0,-398#Rotation:164#Rotation X:-56",
    "2D^Range^Characters,1,2,-97,81,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:36#Skew:0#Skew Axis:78#Rotation:0",
    "2D^Range^Characters,1,2,-90,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:52#Skew Axis:68#Position:-462,-142,0#Anchor Point:130,0,0",
    "3D^Range^Characters,1,2,-74,97,1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:0#Position:0,419,-219#Rotation X:0#Anchor Point:0,-228,0",
    "3D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Position:570,0,0#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-276,0,0#Tracking Type:3#Tracking:-14#Rotation Y:-84#Anchor Point:0,48,0",
    "3D^Range^Characters,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-140#Blur:1,5#Position:503,0,-383",
    "3D^Range^Characters,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-80,-60#Anchor Point:357,471,-129#Rotation X:159#Rotation:-93#Blur:4,0",
    "3D^Range^Lines,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-65#Rotation X:-59#Skew:53#Skew Axis:63#Blur:6,6#Anchor Point:0,-236,0",
    "2D^Range^Characters,1,2,-50,80,2,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,-338,0#Tracking Type:2#Tracking:13#Skew:0#Skew Axis:60",
    "3D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:106#Position:0,64,-164#Blur:6,0",
    "3D^Range^Words,1,2,-81,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Tracking Type:3#Tracking:0#Position:0,571,0#Rotation Y:0#Rotation X:152",
    "2D^Range^Words,1,2,-71,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:13#Skew Axis:65#Position:-553,0,-593#Anchor Point:352,0,0",
    "2D^Range^Lines,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:59#Tracking Type:2#Tracking:59#Anchor Point:0,221,-163",
    "2D^Range^Lines,1,2,-98,73,14,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,2#Position:676,15,0",
    "3D^Range^Characters,1,2,-50,78,13,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:0,0#Rotation Y:0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-47,50,2,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-22",
    "2D^Range^Words,1,2,-43,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:53#Skew Axis:79",
    "3D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-548,0,588#Rotation X:0#Tracking Type:3#Tracking:37#Skew:0#Skew Axis:19",
    "3D^Range^Words,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:41#Rotation:0",
    "3D^Range^Words,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-148#Rotation Y:-107#Blur:20,26#Skew:10#Skew Axis:20",
    "2D^Range^Characters,1,2,-51,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:24#Skew Axis:0",
    "3D^Range^Characters,1,2,-72,97,6,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-90,190#Rotation X:-121#Skew:7#Skew Axis:19#Blur:16,0",
    "2D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:48#Rotation:-32#Position:-424,424,0#Blur:26,0#Skew:10#Skew Axis:23",
    "3D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Rotation Y:-36#Blur:3,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,67,4,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Rotation Y:-174#Rotation X:107#Rotation:43",
    "3D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Rotation Y:109#Rotation X:0#Tracking Type:2#Tracking:59",
    "2D^Range^Characters,1,2,-77,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:86#Blur:3,0#Tracking Type:2#Tracking:-8#Skew:46#Skew Axis:5#Anchor Point:-88,-403,0",
    "3D^Range^Characters,1,2,-50,76,3,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,18#Tracking Type:1#Tracking:0",
    "3D^Range^Lines,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:46#Rotation:154#Skew:54#Skew Axis:70#Position:-478,-378,0",
    "2D^Range^Lines,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,329,-249",
    "3D^Range^Characters,1,2,-95,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:177#Anchor Point:0,-390,0#Rotation X:-171#Rotation Y:0#Position:0,127,0",
    "2D^Range^Characters,1,2,-50,99,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:8#Skew Axis:24#Position:23,-404,0#Tracking Type:1#Tracking:55#Rotation:-127#Blur:6,25",
    "2D^Range^Lines,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:57#Skew Axis:77#Tracking Type:2#Tracking:-39#Rotation:87",
    "2D^Range^Characters,1,2,-50,90,3,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:65#Rotation:0#Position:91,0,0",
    "3D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:48#Skew Axis:0#Position:297,0,0#Rotation:71#Rotation X:0",
    "3D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-5#Rotation Y:96#Blur:0,30#Rotation X:81#Position:0,334,-254",
    "3D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:0#Rotation Y:163#Blur:20,25#Skew:9#Skew Axis:22",
    "3D^Range^Lines,1,2,-97,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:-62#Tracking Type:2#Tracking:56#Rotation Y:-39#Rotation:0",
    "3D^Range^Characters,1,2,-60,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:56#Rotation:-77#Rotation X:41#Skew:24#Skew Axis:59#Anchor Point:270,70,442#Position:-518,0,0#Blur:23,21",
    "2D^Range^Lines,1,2,-77,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:65#Skew Axis:66#Position:0,-595,514#Blur:0,28#Tracking Type:2#Tracking:-7#Rotation:42",
    "3D^Range^Characters,1,2,-56,39,4,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:135#Anchor Point:-153,425,-368#Position:85,0,0",
    "3D^Range^Characters,1,2,-50,77,9,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:-96#Skew:31#Skew Axis:54#Tracking Type:1#Tracking:0",
    "2D^Range^Characters,1,2,-60,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,-370,-355#Position:-688,0,0",
    "3D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-45#Blur:1,5#Rotation X:50#Position:-495,0,0",
    "3D^Range^Characters,1,2,-50,72,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Skew:0#Skew Axis:52#Position:0,0,377#Tracking Type:2#Tracking:0#Rotation Y:112#Rotation X:25",
    "2D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,2#Anchor Point:0,0,-466",
    "3D^Range^Lines,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:100#Skew:0#Skew Axis:24#Tracking Type:2#Tracking:0#Position:490,0,0",
    "2D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:24#Skew Axis:10#Blur:21,5",
    "3D^Range^Characters,1,2,-46,63,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:0#Skew Axis:51#Anchor Point:0,47,0#Position:-266,406,0#Tracking Type:1#Tracking:55#Rotation X:0",
    "3D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:-22#Rotation:42#Rotation X:6#Skew:10#Skew Axis:62#Position:43,0,0",
    "2D^Range^Words,1,2,-79,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:17#Skew Axis:65#Position:0,-590,510",
    "2D^Range^Characters,1,2,-62,50,12,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:38#Position:-307,0,0",
    "3D^Range^Characters,1,2,-90,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:-317,0,0#Position:0,-364,0#Rotation Y:6#Skew:10#Skew Axis:16",
    "3D^Range^Characters,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-103#Rotation:87",
    "3D^Range^Lines,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:49#Skew Axis:1#Rotation Y:49#Anchor Point:0,287,0#Tracking Type:1#Tracking:-55",
    "2D^Range^Characters,1,2,-60,67,7,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:40#Skew Axis:36#Position:0,338,-258",
    "3D^Range^Words,1,2,-49,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:1,26#Rotation X:0#Rotation:16",
    "3D^Range^Characters,1,2,-62,51,10,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:28#Skew Axis:44#Tracking Type:1#Tracking:-57#Rotation X:-73#Anchor Point:0,-218,0#Blur:4,0#Rotation:127",
    "3D^Range^Words,1,2,-50,91,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:0#Tracking Type:2#Tracking:-49#Rotation X:44#Rotation:0#Skew:35#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,-206,0#Position:-428,-488,-468#Rotation:53#Skew:63#Skew Axis:0#Tracking Type:2#Tracking:-9",
    "2D^Range^Lines,1,2,-50,81,3,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:0,-396,0#Rotation:0#Skew:41#Skew Axis:0#Position:214,0,0",
    "3D^Range^Characters,1,2,-95,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:0#Blur:0,28#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-68,38,13,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-47#Position:0,212,0",
    "2D^Range^Characters,1,2,-50,74,4,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0",
    "2D^Range^Characters,1,2,-77,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,358,0#Rotation:0#Skew:61#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,72,13,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:544,0,0#Rotation Y:-114#Anchor Point:-289,-332,0#Rotation:-27#Blur:11,0",
    "3D^Range^Characters,1,2,-50,78,11,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:12#Skew Axis:25#Rotation X:-69#Position:0,-590,0#Rotation Y:-3",
    "3D^Range^Characters,1,2,-74,70,4,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,0#Rotation X:0#Anchor Point:0,-72,-472#Rotation Y:-165#Position:0,0,-371",
    "3D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:491,0,105#Position:467,367,0#Rotation X:-164#Rotation Y:141#Rotation:105",
    "3D^Range^Characters,1,2,-85,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-35",
    "3D^Range^Characters,1,2,-98,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-85#Position:0,71,51#Rotation X:-126#Rotation:-139#Skew:48#Skew Axis:6",
    "3D^Range^Characters,1,2,-65,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:-29#Rotation:0#Skew:5#Skew Axis:24#Position:692,0,152",
    "3D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:363,0,391#Rotation:0#Tracking Type:3#Tracking:0#Rotation X:33",
    "2D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:15,0#Tracking Type:2#Tracking:10#Position:0,-534,0",
    "3D^Range^Words,1,2,-77,74,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Skew:59#Skew Axis:0#Tracking Type:2#Tracking:-6#Rotation X:0#Position:0,0,0",
    "3D^Range^Lines,1,2,-42,65,10,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-59#Skew:53#Skew Axis:63#Blur:26,0",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-247,-647,0#Rotation:172#Skew:17#Skew Axis:15#Rotation Y:-151#Anchor Point:0,-34,-19",
    "3D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:136#Anchor Point:0,393,0#Rotation:28#Skew:0#Skew Axis:0",
    "2D^Range^Lines,1,2,-56,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:125#Skew:8#Skew Axis:6#Blur:13,0#Tracking Type:3#Tracking:26",
    "3D^Range^Words,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Rotation Y:0#Position:0,-513,0#Rotation X:0#Anchor Point:45,102,-459#Rotation:-21",
    "2D^Range^Characters,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:44#Skew Axis:16#Blur:20,10#Position:-23,403,-323#Tracking Type:1#Tracking:-58#Rotation:-45",
    "2D^Range^Characters,1,2,-77,94,2,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-35,0#Position:190,470,449#Rotation:-49#Skew:61#Skew Axis:37",
    "2D^Range^Characters,1,2,-90,99,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Position:266,0,0#Tracking Type:3#Tracking:0#Skew:52#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:-169#Position:-38,0,-138#Skew:32#Skew Axis:42#Blur:26,0",
    "3D^Range^Characters,1,2,-55,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:132",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Skew:39#Skew Axis:0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,30#Position:0,336,0#Skew:33#Skew Axis:55#Tracking Type:2#Tracking:9#Anchor Point:-376,490,0",
    "3D^Range^Characters,1,2,-88,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-63#Skew:55#Skew Axis:49#Position:-417,-677,-137#Tracking Type:1#Tracking:58",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,-353,0#Rotation Y:0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:42#Blur:8,12#Position:0,629,-209#Rotation:-18#Anchor Point:106,421,-449",
    "3D^Range^Words,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:149#Position:0,599,0#Rotation X:164#Rotation:0",
    "2D^Range^Words,1,2,-77,98,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-505,0,605#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:124#Rotation:-57#Blur:0,28#Tracking Type:2#Tracking:-39#Skew:35#Skew Axis:17",
    "3D^Range^Characters,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Position:-162,0,0#Skew:0#Skew Axis:64#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-83,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:30#Blur:24,0",
    "3D^Range^Characters,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,393,33#Tracking Type:2#Tracking:-2",
    "3D^Range^Characters,1,2,-66,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-675,515,-95#Skew:45#Skew Axis:4#Tracking Type:3#Tracking:-48#Rotation Y:-153",
    "3D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:-111#Rotation Y:0#Tracking Type:1#Tracking:0#Rotation:0",
    "2D^Range^Characters,1,2,-50,80,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:167,-127,-647#Anchor Point:19,0,0#Blur:8,0",
    "2D^Range^Characters,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-50#Anchor Point:306,0,0",
    "2D^Range^Lines,1,2,-56,41,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:125,-325,153#Blur:0,7#Skew:37#Skew Axis:35#Position:95,0,74",
    "3D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:0#Rotation X:149#Rotation Y:0#Skew:57#Skew Axis:77#Position:0,-81,0",
    "3D^Range^Characters,1,2,-70,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:143#Blur:0,23#Anchor Point:348,0,376#Tracking Type:2#Tracking:-5#Position:0,-597,0#Skew:54#Skew Axis:42",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-55#Rotation:-68#Skew:2#Skew Axis:60#Anchor Point:273,-416,0",
    "2D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:53#Skew Axis:72#Blur:26,6#Anchor Point:0,-235,0#Tracking Type:3#Tracking:-17",
    "2D^Range^Characters,1,2,-50,100,6,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:39#Blur:0,21",
    "3D^Range^Characters,1,2,-80,86,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-39#Position:-316,0,356#Anchor Point:-183,0,0",
    "3D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:22#Rotation:0#Position:-241,121,221#Rotation X:-52#Blur:0,26",
    "2D^Range^Lines,1,2,-71,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,18#Position:113,-113,0#Rotation:-129",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0#Tracking Type:3#Tracking:-8#Blur:21,26",
    "3D^Range^Lines,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:664,364,0#Skew:14#Skew Axis:12#Rotation Y:83",
    "3D^Range^Lines,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-53#Anchor Point:461,-333,0#Rotation X:89#Rotation:-22",
    "3D^Range^Words,1,2,-50,85,11,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:147#Rotation Y:0#Rotation:0#Tracking Type:3#Tracking:46",
    "2D^Range^Characters,1,2,-49,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:-48#Position:0,126,0",
    "3D^Range^Lines,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,0#Rotation Y:0#Anchor Point:0,-77,-476",
    "2D^Range^Characters,1,2,-90,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-73#Tracking Type:3#Tracking:-48",
    "2D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:21#Rotation:0#Skew:63#Skew Axis:0#Blur:14,0#Position:0,138,0",
    "3D^Range^Characters,1,2,-50,71,6,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Rotation:0#Blur:1,5#Rotation X:51#Position:499,0,-599#Skew:0#Skew Axis:0",
    "3D^Range^Words,1,2,-95,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:1,5#Rotation X:50#Position:494,0,374",
    "2D^Range^Words,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-31,0,0#Skew:23#Skew Axis:9",
    "2D^Range^Characters,1,2,-41,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:-402,273,259",
    "3D^Range^Lines,1,2,-37,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-118#Tracking Type:3#Tracking:6#Rotation Y:128#Rotation X:179#Skew:8#Skew Axis:18",
    "3D^Range^Words,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:0#Rotation:0",
    "3D^Range^Lines,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,389,0#Rotation:0#Skew:40#Skew Axis:0#Tracking Type:1#Tracking:0",
    "2D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-180,0#Skew:60#Skew Axis:0#Blur:3,0#Anchor Point:0,122,0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:-19#Position:296,0,336#Rotation:0#Anchor Point:0,-404,-147",
    "2D^Range^Words,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:177,0,0#Position:0,648,-228#Rotation:23#Skew:45#Skew Axis:3#Tracking Type:3#Tracking:-48",
    "3D^Range^Words,1,2,-53,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-158#Tracking Type:1#Tracking:24",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:-350,7,0#Tracking Type:2#Tracking:0#Position:-570,0,0#Rotation Y:0",
    "3D^Range^Characters,1,2,-50,97,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Blur:0,24",
    "2D^Range^Characters,1,2,-69,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,376,361#Tracking Type:3#Tracking:37#Position:166,0,0#Skew:53#Skew Axis:5",
    "3D^Range^Characters,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:124#Skew:8#Skew Axis:18#Tracking Type:1#Tracking:52",
    "2D^Range^Characters,1,2,-90,88,11,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,183,0#Skew:60#Skew Axis:0#Rotation:-75#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-89,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:5#Blur:0,20",
    "2D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:405,0,625#Rotation:27#Skew:6#Skew Axis:33#Tracking Type:2#Tracking:26",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:0#Blur:0,0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:0#Rotation:8#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-59,50,14,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,-369,-355#Rotation:61#Blur:0,29#Tracking Type:3#Tracking:0#Skew:20#Skew Axis:6",
    "3D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:-556,0,0#Blur:0,27#Skew:0#Skew Axis:0",
    "3D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:58#Rotation X:75#Anchor Point:0,222,-8#Skew:0#Skew Axis:46#Rotation:52",
    "3D^Range^Characters,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:46#Skew Axis:11#Blur:20,5#Rotation X:161",
    "3D^Range^Lines,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:-20#Tracking Type:2#Tracking:7#Skew:0#Skew Axis:0#Position:0,617,-257#Rotation X:25",
    "3D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0#Rotation Y:0#Rotation X:-3",
    "3D^Range^Characters,1,2,-73,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:16,6#Tracking Type:1#Tracking:-39#Skew:40#Skew Axis:32",
    "2D^Range^Characters,1,2,-50,95,4,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:30#Skew Axis:21#Blur:16,6#Position:-230,370,-70",
    "3D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:2#Skew Axis:0#Rotation X:71#Rotation Y:48#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:326,0,0#Blur:11,6#Skew:32#Skew Axis:42",
    "2D^Range^Words,1,2,-100,55,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:19#Position:-301,0,0#Skew:48#Skew Axis:32#Anchor Point:101,-72,-101",
    "2D^Range^Characters,1,2,-56,39,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:1#Tracking:-37#Skew:34#Skew Axis:43#Position:-277,0,0",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-176#Position:-543,0,583#Anchor Point:-345,0,0",
    "2D^Range^Lines,1,2,-83,92,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Skew:52#Skew Axis:78#Position:0,0,-359",
    "2D^Range^Characters,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:130",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:23#Skew Axis:51#Anchor Point:267,-410,439#Position:514,0,-554#Rotation:-107",
    "3D^Range^Characters,1,2,-39,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,221,7#Rotation X:-123#Blur:0,19",
    "2D^Range^Characters,1,2,-76,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:29#Skew Axis:45#Tracking Type:1#Tracking:57#Anchor Point:0,219,-5",
    "2D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:667,-487,247#Blur:0,29#Tracking Type:1#Tracking:39#Skew:36#Skew Axis:20#Rotation:-51",
    "3D^Range^Characters,1,2,-50,87,1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:61#Skew Axis:65#Position:-552,0,0#Blur:5,0#Rotation X:-83",
    "2D^Range^Characters,1,2,-61,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:0#Rotation:-28#Skew:53#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,95,9,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:37#Rotation X:23#Rotation Y:167#Rotation:-131#Position:0,-169,0",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:7,0",
    "3D^Range^Characters,1,2,-50,97,6,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation X:0#Skew:64#Skew Axis:0#Blur:23,0#Tracking Type:2#Tracking:0#Position:0,0,0",
    "2D^Range^Characters,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:0#Blur:0,0#Tracking Type:3#Tracking:-47#Anchor Point:0,-406,0",
    "3D^Range^Characters,1,2,-50,67,3,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:119#Position:-325,0,0#Tracking Type:2#Tracking:19#Rotation Y:78#Skew:57#Skew Axis:77#Rotation:125",
    "3D^Range^Characters,1,2,-88,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:45#Blur:1,25#Rotation X:-50#Position:-495,0,-375",
    "3D^Range^Words,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:53#Anchor Point:0,-57,0#Tracking Type:1#Tracking:-57#Rotation X:16",
    "2D^Range^Lines,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-289,332,0#Rotation:-27#Blur:21,0#Tracking Type:2#Tracking:2#Position:-465,-525,0",
    "2D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,140,0#Blur:0,19#Tracking Type:1#Tracking:0#Position:536,-176,0#Rotation:4",
    "2D^Range^Characters,1,2,-82,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:2#Tracking:-59#Anchor Point:0,-216,0#Blur:0,0#Rotation:0#Skew:0#Skew Axis:23#Position:0,173,-32",
    "3D^Range^Words,1,2,-48,45,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:177#Skew:7#Skew Axis:17#Blur:16,0#Tracking Type:2#Tracking:-6",
    "2D^Range^Lines,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:8#Skew Axis:27#Tracking Type:1#Tracking:-60#Anchor Point:0,227,0#Blur:26,0",
    "3D^Range^Lines,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-111#Position:-293,0,-333#Tracking Type:1#Tracking:0#Skew:0#Skew Axis:76",
    "2D^Range^Characters,1,2,-83,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,0,-475#Rotation:55#Blur:0,28#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Rotation:0",
    "3D^Range^Characters,1,2,-97,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:43#Anchor Point:333,0,0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,81,7,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:174#Position:-56,0,-576#Blur:22,19#Anchor Point:-326,0,354",
    "3D^Range^Characters,1,2,-61,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:16#Skew Axis:14#Position:581,0,-561#Rotation X:-77#Anchor Point:0,29,-15#Tracking Type:3#Tracking:-29",
    "3D^Range^Characters,1,2,-88,79,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-43#Skew:0#Skew Axis:24#Rotation:0#Position:0,596,0#Rotation X:56",
    "3D^Range^Words,1,2,-79,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-307,-367,0#Tracking Type:1#Tracking:0#Rotation Y:48#Rotation X:38",
    "3D^Range^Lines,1,2,-98,87,15,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:9#Tracking Type:2#Tracking:0#Position:3,-523,-503",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:1#Skew Axis:0#Rotation:-39#Tracking Type:2#Tracking:0",
    "2D^Range^Words,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:43#Skew Axis:40#Position:0,-369,288#Anchor Point:0,-220,0#Blur:5,0",
    "3D^Range^Characters,1,2,-48,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:0#Position:0,-374,474#Rotation X:-173",
    "3D^Range^Characters,1,2,-61,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,0,0",
    "3D^Range^Words,1,2,-92,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:3,17#Rotation X:180#Skew:8#Skew Axis:20#Rotation Y:-123#Position:328,0,0",
    "3D^Range^Lines,1,2,-96,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-32#Rotation X:-148",
    "3D^Range^Words,1,2,-98,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:503,403,103#Skew:42#Skew Axis:34#Rotation X:73#Anchor Point:0,216,159",
    "2D^Range^Words,1,2,-50,99,2,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-338,38#Rotation:131",
    "3D^Range^Lines,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,49,335#Rotation:46#Skew:60#Skew Axis:0#Tracking Type:2#Tracking:-7#Rotation X:0#Position:0,0,0",
    "2D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,6#Skew:53#Skew Axis:47",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:7,0#Anchor Point:0,-407,-150#Skew:0#Skew Axis:33#Position:220,0,0",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,18#Skew:15#Skew Axis:61#Position:570,0,-330#Rotation X:13#Rotation Y:-170#Rotation:10",
    "2D^Range^Lines,1,2,-94,74,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-575,0",
    "2D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:23#Skew Axis:51#Anchor Point:-261,61,433",
    "3D^Range^Characters,1,2,-41,61,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Position:0,0,401#Rotation X:136",
    "2D^Range^Characters,1,2,-60,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:28#Skew Axis:44#Position:-215,0,0#Tracking Type:1#Tracking:-42#Anchor Point:289,332,318",
    "3D^Range^Characters,1,2,-87,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:60#Anchor Point:0,225,0#Rotation X:0#Position:0,-415,215#Blur:23,8#Rotation:0",
    "3D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Skew:0#Skew Axis:0#Rotation:-144#Blur:0,18",
    "3D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:94#Rotation X:37",
    "2D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-70,64,0,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:16#Skew:60#Skew Axis:66#Position:321,-21,0#Blur:0,12#Rotation:131",
    "2D^Range^Lines,1,2,-50,87,3,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-41#Anchor Point:287,-487,316#Skew:18#Skew Axis:16",
    "2D^Range^Lines,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:-90,-62,90#Position:-546,0,0#Blur:30,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-88,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-121#Skew:7#Skew Axis:20#Blur:16,0#Position:242,0,0",
    "2D^Range^Characters,1,2,-83,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,170,0#Skew:59#Skew Axis:63",
    "2D^Range^Characters,1,2,-67,61,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:10,5#Position:296,0,-396#Skew:8#Skew Axis:20",
    "2D^Range^Characters,1,2,-50,66,14,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:-104,-418,0#Blur:0,0#Skew:32#Skew Axis:22#Rotation:102#Tracking Type:1#Tracking:0",
    "2D^Range^Words,1,2,-50,94,10,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:47#Position:30,0,-130#Skew:31#Skew Axis:41#Blur:26,0",
    "2D^Range^Words,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:325,54,0",
    "3D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:0#Position:0,-106,0#Skew:31#Skew Axis:53#Rotation:0",
    "3D^Range^Lines,1,2,-65,64,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-82#Position:0,181,-461#Anchor Point:-86,-57,0#Blur:0,3#Tracking Type:2#Tracking:0#Rotation X:0",
    "3D^Range^Characters,1,2,-60,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-95#Tracking Type:2#Tracking:49#Position:501,401,-321#Skew:64#Skew Axis:34",
    "2D^Range^Characters,1,2,-50,87,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:-465,-45,0",
    "2D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:-290,0,0#Skew:69#Skew Axis:31",
    "3D^Range^Words,1,2,-60,47,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-49#Anchor Point:451,-323,-308#Tracking Type:3#Tracking:-16#Rotation:91",
    "3D^Range^Words,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation X:0#Rotation:0#Blur:19,10#Skew:0#Skew Axis:0#Position:0,0,0",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:6,0#Skew:55#Skew Axis:74#Rotation Y:0#Tracking Type:1#Tracking:-56",
    "3D^Range^Characters,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:11#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-52,43,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:528,0,0#Rotation:23#Skew:51#Skew Axis:67#Tracking Type:1#Tracking:-37",
    "3D^Range^Characters,1,2,-89,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:1,5#Rotation X:53#Position:507,0,-607",
    "2D^Range^Characters,1,2,-50,84,11,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:683,-503,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0#Rotation:168",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:150#Blur:11,0#Rotation X:-88#Tracking Type:2#Tracking:0",
    "2D^Range^Words,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:159#Tracking Type:1#Tracking:-50#Skew:0#Skew Axis:34#Position:-498,0,0",
    "2D^Range^Characters,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:10#Skew Axis:16#Position:-36,0,0#Blur:23,12",
    "2D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,0,1",
    "2D^Range^Lines,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:9#Skew Axis:28#Rotation:146#Position:-506,-406,0#Anchor Point:319,0,433",
    "2D^Range^Characters,1,2,-80,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-454,325,0#Rotation:-30#Blur:18,0",
    "3D^Range^Words,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:53#Anchor Point:0,58,0#Tracking Type:1#Tracking:-57#Rotation X:0",
    "2D^Range^Words,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-28#Skew:53#Skew Axis:79#Tracking Type:1#Tracking:-57#Blur:0,30",
    "3D^Range^Words,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:-20#Anchor Point:0,0,0#Tracking Type:3#Tracking:16#Position:-306,0,-126#Rotation X:0#Skew:23#Skew Axis:0",
    "2D^Range^Characters,1,2,-51,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,9#Tracking Type:2#Tracking:18#Position:-511,0,611",
    "2D^Range^Characters,1,2,-47,50,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-640,-140,0#Rotation:-77#Skew:24#Skew Axis:52#Anchor Point:271,71,0",
    "3D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-107#Rotation:91#Skew:29#Skew Axis:52",
    "2D^Range^Characters,1,2,-91,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-72#Position:0,421,221#Skew:0#Skew Axis:0#Anchor Point:0,0,0",
    "3D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:0#Anchor Point:0,-90,332#Skew:67#Skew Axis:35",
    "3D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:102#Rotation X:0#Rotation Y:0",
    "3D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:180#Position:-439,-19,0",
    "3D^Range^Characters,1,2,-67,59,7,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:0,13#Rotation:-159#Rotation X:-102#Position:497,397,-97#Skew:42#Skew Axis:34",
    "2D^Range^Characters,1,2,-50,75,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:0,0,186#Position:0,411,-691#Blur:21,26#Skew:10#Skew Axis:23",
    "2D^Range^Words,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-235,0#Position:219,0,0",
    "3D^Range^Characters,1,2,-70,59,3,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,0,-603#Rotation:0#Anchor Point:0,45,0",
    "2D^Range^Words,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,591,0#Skew:31#Skew Axis:54#Rotation:-23",
    "2D^Range^Characters,1,2,-50,82,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:16#Position:304,0,0#Skew:48#Skew Axis:33",
    "2D^Range^Characters,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:91#Tracking Type:2#Tracking:47#Blur:1,6#Position:513,0,-393",
    "2D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-417,-317,0#Anchor Point:0,448,-105#Rotation:28#Blur:23,0#Tracking Type:2#Tracking:-9",
    "2D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:-676,496,0#Skew:19#Skew Axis:1#Rotation:0#Tracking Type:3#Tracking:7",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:1#Tracking:0#Rotation:-28#Blur:16,9#Rotation Y:0",
    "3D^Range^Characters,1,2,-72,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:45#Rotation Y:7#Skew:11#Skew Axis:21#Rotation X:-77#Anchor Point:0,228,14",
    "3D^Range^Characters,1,2,-50,75,0,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-158#Tracking Type:2#Tracking:-17#Rotation Y:70#Blur:0,7#Position:0,-42,0",
    "2D^Range^Characters,1,2,-80,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:413,-693,0#Rotation:-29#Blur:23,0",
    "3D^Range^Characters,1,2,-37,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:-174#Skew:0#Skew Axis:50#Blur:29,10#Tracking Type:2#Tracking:60#Anchor Point:0,-227,0",
    "2D^Range^Characters,1,2,-66,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:0#Position:0,116,-35#Blur:8,0",
    "2D^Range^Words,1,2,-50,85,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:0#Skew:0#Skew Axis:0#Blur:16,0#Rotation:0",
    "3D^Range^Characters,1,2,-66,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:41#Position:0,97,-498#Rotation:-110#Skew:15#Skew Axis:15#Rotation Y:141",
    "2D^Range^Characters,1,2,-83,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:103#Anchor Point:-1,0,72#Position:281,0,0",
    "3D^Range^Lines,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:31#Blur:24,0#Tracking Type:2#Tracking:0#Skew:26#Skew Axis:0#Rotation X:0#Anchor Point:464,0,-493",
    "3D^Range^Characters,1,2,-73,74,0,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:13,27#Tracking Type:2#Tracking:-17#Rotation Y:17#Skew:14#Skew Axis:18#Position:84,0,604",
    "2D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:21,5#Skew:0#Skew Axis:37#Rotation:134#Tracking Type:3#Tracking:8",
    "3D^Range^Lines,1,2,-44,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,-345,0#Rotation Y:0#Skew:59#Skew Axis:0",
    "2D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:2#Skew Axis:60#Anchor Point:274,-417,0#Blur:2,17",
    "3D^Range^Characters,1,2,-79,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:28#Blur:11,0#Tracking Type:2#Tracking:-1",
    "3D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:-104,75,447#Rotation Y:-135#Tracking Type:3#Tracking:0#Blur:0,0",
    "2D^Range^Characters,1,2,-69,42,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Tracking Type:2#Tracking:0#Position:0,-70,0",
    "3D^Range^Characters,1,2,-99,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-12#Tracking Type:2#Tracking:20#Anchor Point:0,177,0",
    "3D^Range^Lines,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-85#Skew:0#Skew Axis:47#Position:0,352,-52#Blur:0,30",
    "2D^Range^Words,1,2,-74,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:15#Skew Axis:28#Anchor Point:161,0,0#Tracking Type:3#Tracking:-45#Position:286,0,-385",
    "2D^Range^Words,1,2,-41,89,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:0#Skew:21#Skew Axis:0#Position:-150,0,0",
    "3D^Range^Lines,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Position:357,0,337#Rotation X:0",
    "3D^Range^Words,1,2,-71,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:47#Rotation:13#Rotation Y:-152#Position:431,11,0",
    "3D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:7#Skew Axis:26#Tracking Type:1#Tracking:-59#Anchor Point:0,222,-7#Rotation Y:-85",
    "3D^Range^Words,1,2,-61,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0",
    "2D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,387,0#Rotation:26#Skew:52#Skew Axis:78#Tracking Type:1#Tracking:57#Blur:29,10",
    "3D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:115#Anchor Point:-133,-333,-318#Skew:19#Skew Axis:65#Tracking Type:2#Tracking:56#Position:0,309,-349#Rotation:110",
    "3D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,19#Skew:15#Skew Axis:0",
    "2D^Range^Characters,1,2,-48,89,11,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:23#Skew Axis:65",
    "2D^Range^Characters,1,2,-50,73,6,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:37#Position:-444,0,544#Rotation:0#Skew:0#Skew Axis:0",
    "2D^Range^Words,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:63#Skew Axis:45#Position:-552,0,-532",
    "2D^Range^Characters,1,2,-50,90,5,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:36#Skew Axis:20#Tracking Type:2#Tracking:37#Position:276,0,436#Rotation:-97",
    "3D^Range^Lines,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:-35#Blur:0,19#Rotation X:-143#Position:0,95,-495#Rotation:-107#Skew:67#Skew Axis:31",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-115#Position:308,0,348#Anchor Point:-177,0,0#Rotation Y:0",
    "2D^Range^Words,1,2,-76,80,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:10#Skew Axis:0#Rotation:0#Blur:0,0",
    "3D^Range^Words,1,2,-69,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:16#Skew Axis:16#Position:-581,0,-561#Anchor Point:0,-30,0",
    "3D^Range^Words,1,2,-64,55,7,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-400,0#Rotation X:-118",
    "2D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:0,216,0#Blur:4,0",
    "3D^Range^Lines,1,2,-52,50,5,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0#Tracking Type:2#Tracking:0#Rotation Y:0#Blur:2,27#Rotation X:0",
    "2D^Range^Lines,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-6#Skew:23#Skew Axis:58#Anchor Point:-418,61,0",
    "2D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-513,0,613#Blur:25,5#Anchor Point:102,331,0#Tracking Type:2#Tracking:35",
    "2D^Range^Lines,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-49#Skew:30#Skew Axis:52#Position:0,0,377#Anchor Point:0,227,12",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,-474,574#Tracking Type:2#Tracking:0#Rotation:96#Anchor Point:217,-231,-346#Blur:0,0#Skew:23#Skew Axis:11",
    "3D^Range^Lines,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Tracking Type:3#Tracking:0#Skew:0#Skew Axis:48#Position:-551,251,-471#Rotation Y:54",
    "3D^Range^Words,1,2,-85,73,3,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-436,456,616#Tracking Type:1#Tracking:-60#Anchor Point:0,-225,11#Rotation Y:-35#Blur:7,0",
    "3D^Range^Lines,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-534,0#Rotation Y:0#Rotation X:-4",
    "3D^Range^Characters,1,2,-50,74,3,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Rotation:74",
    "2D^Range^Characters,1,2,-64,64,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:446,0,0#Blur:11,0",
    "3D^Range^Words,1,2,-50,93,4,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:146#Position:0,589,-508#Skew:22#Skew Axis:78#Tracking Type:3#Tracking:7#Rotation:-41",
    "3D^Range^Characters,1,2,-93,79,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:111#Rotation:96#Skew:0#Skew Axis:52#Blur:0,20",
    "3D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:1#Tracking:38#Skew:0#Skew Axis:0#Rotation:0#Rotation X:33",
    "3D^Range^Words,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Rotation:176#Position:-544,0,-584#Anchor Point:346,0,0",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-187,-47#Rotation:-7#Tracking Type:1#Tracking:31#Blur:29,10",
    "3D^Range^Characters,1,2,-96,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:20,11#Position:-25,-406,0#Tracking Type:1#Tracking:0",
    "3D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,274,0#Anchor Point:0,210,-196#Tracking Type:2#Tracking:0#Rotation:-140",
    "2D^Range^Characters,1,2,-50,85,12,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,362,348#Tracking Type:3#Tracking:15#Blur:0,7",
    "3D^Range^Characters,1,2,-36,64,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:0#Rotation:0#Blur:1,26#Tracking Type:2#Tracking:-49",
    "3D^Range^Words,1,2,-50,89,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:0#Rotation:159#Skew:56#Skew Axis:2#Position:0,0,-397",
    "3D^Range^Characters,1,2,-50,90,8,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:9,10#Rotation X:45#Rotation:0",
    "2D^Range^Words,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-639,39,619#Skew:12#Skew Axis:25",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:146#Blur:0,12#Position:268,0,-368#Rotation X:172",
    "3D^Range^Words,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:53#Skew Axis:79#Position:471,370,71#Rotation X:-80",
    "2D^Range^Characters,1,2,-50,89,8,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:191,0,-220#Blur:0,20#Rotation:0#Tracking Type:1#Tracking:0",
    "3D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:29#Blur:11,0#Tracking Type:1#Tracking:30",
    "3D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:-77#Position:0,-111,511",
    "2D^Range^Characters,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0",
    "2D^Range^Words,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:137",
    "3D^Range^Characters,1,2,-67,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:16,0#Skew:36#Skew Axis:53#Position:0,-351,51#Rotation X:-11",
    "2D^Range^Words,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:13#Skew Axis:11#Position:-554,0,0#Anchor Point:-453,324,-153#Tracking Type:3#Tracking:-40#Rotation:-125",
    "3D^Range^Words,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:4,0#Tracking Type:3#Tracking:-10#Rotation Y:142#Anchor Point:0,366,-194#Position:0,-152,0#Skew:15#Skew Axis:70",
    "2D^Range^Characters,1,2,-50,75,0,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:0#Tracking Type:2#Tracking:-5#Position:244,0,-465",
    "3D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Skew:61#Skew Axis:0#Position:0,0,142#Rotation:0",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-45#Anchor Point:0,407,-149#Skew:0#Skew Axis:33#Position:489,0,-589#Rotation X:0",
    "3D^Range^Characters,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:-32#Position:563,0,543#Anchor Point:-202,-488,0#Rotation:1#Skew:51#Skew Axis:67",
    "2D^Range^Characters,1,2,-96,92,4,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-17#Position:281,0,101",
    "2D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-155#Tracking Type:1#Tracking:-30#Position:0,-304,-524",
    "2D^Range^Characters,1,2,-58,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-31",
    "3D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:-29#Position:555,0,0#Anchor Point:-418,0,446#Skew:0#Skew Axis:0",
    "2D^Range^Words,1,2,-50,65,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:6#Skew Axis:22#Tracking Type:1#Tracking:58#Position:113,0,0",
    "2D^Range^Words,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:104#Anchor Point:4,0,-76#Position:-486,0,0",
    "3D^Range^Characters,1,2,-50,87,15,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:27#Position:-543,0,304#Anchor Point:0,-31,0",
    "2D^Range^Characters,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:292,0,-392#Skew:7#Skew Axis:17#Blur:16,6",
    "2D^Range^Words,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-35#Position:-274,0,-314",
    "3D^Range^Words,1,2,-94,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:37#Skew:33#Skew Axis:43#Position:273,0,0",
    "3D^Range^Characters,1,2,-50,83,5,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:29,10#Rotation Y:0#Anchor Point:0,231,0#Rotation X:0#Position:-93,0,-73",
    "2D^Range^Words,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:299,0,0#Rotation:0",
    "2D^Range^Characters,1,2,-50,72,4,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:54#Tracking Type:1#Tracking:0",
    "2D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-88,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation Y:-56#Position:658,698,0#Rotation:0#Skew:60#Skew Axis:0#Tracking Type:2#Tracking:0",
    "3D^Range^Words,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-341,0#Rotation X:-175#Rotation Y:-119",
    "2D^Range^Words,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:5,10#Position:0,-98,18#Skew:21#Skew Axis:19#Tracking Type:3#Tracking:-24",
    "2D^Range^Words,1,2,-96,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-167#Anchor Point:-256,241,0#Tracking Type:2#Tracking:24",
    "2D^Range^Words,1,2,-99,73,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:0,0,0#Skew:10#Skew Axis:0#Rotation:3#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0#Tracking Type:1#Tracking:58#Blur:0,30",
    "2D^Range^Characters,1,2,-82,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-682,502,-482#Anchor Point:0,-344,-87",
    "3D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:33#Rotation Y:-134#Tracking Type:3#Tracking:-9",
    "3D^Range^Words,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:13#Skew Axis:0#Position:0,183,0#Rotation X:0",
    "2D^Range^Characters,1,2,-56,59,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,17#Position:289,0,0#Tracking Type:2#Tracking:-16",
    "2D^Range^Words,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-130#Position:-176,0,0",
    "3D^Range^Words,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:54#Rotation Y:18#Rotation:-38#Skew:9#Skew Axis:70",
    "3D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:48#Rotation Y:0#Position:406,126,0#Skew:23#Skew Axis:0",
    "3D^Range^Words,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:1#Tracking:6#Anchor Point:0,-364,0#Rotation X:162",
    "3D^Range^Characters,1,2,-50,98,8,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-477,0,357#Rotation X:-169#Blur:0,6#Skew:54#Skew Axis:73",
    "3D^Range^Characters,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:26#Position:0,0,645",
    "2D^Range^Characters,1,2,-46,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-587,0,-347#Anchor Point:0,-34,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:-116#Skew:17#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,83,11,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:39",
    "3D^Range^Characters,1,2,-63,50,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:0#Rotation:-10#Rotation X:149",
    "2D^Range^Words,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:356,0,0#Tracking Type:3#Tracking:38#Skew:34#Skew Axis:43#Position:278,0,98#Rotation:-46",
    "2D^Range^Lines,1,2,-37,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:461,41,0#Rotation:31#Blur:24,0",
    "3D^Range^Words,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:1#Tracking:0#Skew:54#Skew Axis:73#Blur:0,0#Anchor Point:0,241,484#Rotation Y:46",
    "3D^Range^Characters,1,2,-76,77,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:22,21#Skew:34#Skew Axis:43#Position:0,-430,0",
    "3D^Range^Characters,1,2,-50,73,12,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:174#Position:-58,0,0#Rotation:66#Tracking Type:3#Tracking:29#Rotation Y:138#Skew:12#Skew Axis:25",
    "3D^Range^Characters,1,2,-50,87,6,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,-124,-525#Anchor Point:0,46,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:10,2#Rotation X:71#Rotation Y:164#Skew:50#Skew Axis:30#Position:-157,117,157#Tracking Type:2#Tracking:-49",
    "2D^Range^Characters,1,2,-61,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:0,117,232#Rotation:27#Skew:0#Skew Axis:49",
    "2D^Range^Characters,1,2,-50,85,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,0,142#Rotation:0#Anchor Point:301,0,415#Blur:15,0",
    "2D^Range^Characters,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-354,54#Rotation:45#Blur:0,16",
    "3D^Range^Lines,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:0,363,-283#Anchor Point:0,216,-159#Rotation:60",
    "2D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:49#Skew Axis:7#Position:154,114,0#Blur:20,15#Anchor Point:0,-246,0#Tracking Type:1#Tracking:-16",
    "3D^Range^Characters,1,2,-50,72,13,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-191,0#Skew:61#Skew Axis:0#Rotation:0#Anchor Point:0,-51,0",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-23#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0#Position:0,0,0",
    "3D^Range^Characters,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Skew:0#Skew Axis:26#Rotation:0",
    "3D^Range^Lines,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation Y:0#Skew:25#Skew Axis:0#Rotation:-171#Position:137,0,0#Rotation X:0",
    "3D^Range^Characters,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:46#Blur:1,5#Rotation X:52#Position:-501,0,601#Skew:0#Skew Axis:0",
    "3D^Range^Words,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Blur:0,0#Position:-17,0,0#Skew:22#Skew Axis:9#Rotation Y:159#Anchor Point:-305,0,0",
    "2D^Range^Characters,1,2,-50,92,3,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:335,0,-406#Position:-49,0,0#Rotation:-64#Tracking Type:2#Tracking:-28#Skew:53#Skew Axis:72",
    "3D^Range^Characters,1,2,-39,83,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:-361,0,0#Tracking Type:3#Tracking:36#Position:-286,0,0",
    "3D^Range^Words,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:-82,0,0#Rotation Y:0#Anchor Point:-173,0,0#Tracking Type:3#Tracking:-11",
    "2D^Range^Lines,1,2,-98,78,11,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:0,30#Position:0,333,-33",
    "3D^Range^Characters,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Rotation X:0#Tracking Type:3#Tracking:-49#Rotation Y:-74#Position:-487,0,0",
    "2D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-214,0,0#Blur:10,2#Rotation:-124#Tracking Type:3#Tracking:-48#Skew:10#Skew Axis:10#Anchor Point:0,374,0",
    "3D^Range^Characters,1,2,-63,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-36#Position:-315,0,0#Blur:1,26",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:7,0#Skew:56#Skew Axis:50#Anchor Point:-222,0,0#Tracking Type:2#Tracking:-56",
    "2D^Range^Lines,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:50#Skew Axis:2#Anchor Point:-291,134,-248",
    "2D^Range^Characters,1,2,-49,65,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:278,364,435#Tracking Type:2#Tracking:49",
    "3D^Range^Lines,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-364,64#Anchor Point:0,-217,0",
    "3D^Range^Words,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:26#Position:0,121,522#Anchor Point:0,-44,0#Tracking Type:1#Tracking:55#Rotation:-67",
    "2D^Range^Characters,1,2,-50,85,0,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:-98#Skew:58#Skew Axis:0#Blur:25,0",
    "3D^Range^Characters,1,2,-79,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,9#Tracking Type:1#Tracking:5#Rotation:-170",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:9#Position:-546,0,0#Rotation Y:171#Anchor Point:-290,-490,318#Skew:19#Skew Axis:0#Rotation:84",
    "2D^Range^Characters,1,2,-72,99,9,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:32#Skew Axis:42#Position:0,-227,0#Blur:10,0",
    "2D^Range^Words,1,2,-99,89,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:31#Rotation:0#Position:-576,0,-616#Anchor Point:0,-219,290",
    "2D^Range^Words,1,2,-98,80,10,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-24#Rotation:0",
    "2D^Range^Characters,1,2,-56,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:1#Tracking:58#Anchor Point:0,227,0#Blur:5,0",
    "2D^Range^Characters,1,2,-75,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:21#Tracking Type:2#Tracking:0#Position:220,-40,500",
    "3D^Range^Characters,1,2,-93,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:128#Tracking Type:1#Tracking:-15#Skew:62#Skew Axis:43#Position:-336,0,0",
    "3D^Range^Characters,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:6,17#Rotation:-87#Skew:28#Skew Axis:44",
    "2D^Range^Characters,1,2,-50,77,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:0,-232,-18#Rotation:0#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-72,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:0,-409,-66#Tracking Type:2#Tracking:10",
    "2D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:26#Skew Axis:61#Position:-615,115,0#Rotation:0",
    "3D^Range^Characters,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:0#Anchor Point:-281,0,181",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:16,0#Skew:35#Skew Axis:45#Rotation:-123#Position:309,-9,229",
    "3D^Range^Lines,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-26#Skew:4#Skew Axis:30#Rotation Y:57#Position:-662,-2,462#Blur:0,9",
    "2D^Range^Words,1,2,-84,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-5,525,0#Anchor Point:0,346,0#Rotation:91",
    "2D^Range^Characters,1,2,-67,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:61#Skew Axis:65#Position:547,0,0#Blur:5,9",
    "2D^Range^Words,1,2,-81,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:150#Skew:52#Skew Axis:68#Position:-464,-364,0",
    "3D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,87,0#Rotation X:0",
    "2D^Range^Words,1,2,-60,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:18#Position:284,0,0#Skew:68#Skew Axis:26",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-65#Tracking Type:3#Tracking:-28#Rotation Y:137#Skew:11#Skew Axis:24",
    "2D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:11,26#Skew:10#Skew Axis:23#Tracking Type:1#Tracking:25#Position:0,375,-75",
    "2D^Range^Characters,1,2,-41,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:651,-471,-451#Skew:17#Skew Axis:63",
    "3D^Range^Words,1,2,-91,88,10,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-51#Rotation X:-35#Tracking Type:3#Tracking:17",
    "3D^Range^Characters,1,2,-69,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-123#Skew:8#Skew Axis:21#Blur:7,0#Rotation Y:139#Rotation:-36",
    "2D^Range^Characters,1,2,-70,54,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,456,0#Tracking Type:3#Tracking:47",
    "2D^Range^Characters,1,2,-86,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-158#Skew:55#Skew Axis:1",
    "3D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-58#Rotation X:-40#Skew:56#Skew Axis:75#Rotation:-120",
    "2D^Range^Characters,1,2,-97,85,4,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:71#Anchor Point:333,0,247",
    "3D^Range^Lines,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-165#Anchor Point:0,366,0",
    "2D^Range^Characters,1,2,-95,83,13,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:7,0#Skew:35#Skew Axis:45#Anchor Point:0,389,0",
    "3D^Range^Lines,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Blur:0,16#Skew:34#Skew Axis:51#Tracking Type:3#Tracking:-49#Anchor Point:0,-187,344#Rotation Y:0#Rotation X:-6",
    "2D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:7#Skew Axis:20#Blur:7,0#Position:184,0,0",
    "2D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:68#Skew Axis:0",
    "3D^Range^Characters,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:38#Rotation Y:-5#Skew:22#Skew Axis:56#Rotation:-15",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-67#Tracking Type:2#Tracking:-47#Skew:3#Skew Axis:25",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:26#Skew Axis:61#Position:616,-116,0#Rotation:-14#Anchor Point:97,411,-440",
    "3D^Range^Characters,1,2,-64,45,9,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:352,0,0#Tracking Type:2#Tracking:-16#Rotation:-129#Position:0,-74,0",
    "2D^Range^Lines,1,2,-74,85,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:69#Anchor Point:0,0,-12#Tracking Type:3#Tracking:0",
    "2D^Range^Characters,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:16,0#Rotation:27#Tracking Type:2#Tracking:-28#Anchor Point:0,45,0",
    "2D^Range^Characters,1,2,-50,95,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:0#Skew Axis:0#Position:-305,0,-345#Anchor Point:175,0,-204#Tracking Type:1#Tracking:-29",
    "2D^Range^Lines,1,2,-90,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-309,0,-423#Position:0,-282,0#Tracking Type:1#Tracking:-11",
    "3D^Range^Characters,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Rotation X:-167",
    "2D^Range^Characters,1,2,-52,46,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:55#Skew Axis:74#Position:0,120,0",
    "2D^Range^Characters,1,2,-54,99,11,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,-234,0#Rotation:-174",
    "3D^Range^Characters,1,2,-85,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-173#Blur:18,0#Skew:0#Skew Axis:61#Rotation Y:-60",
    "2D^Range^Characters,1,2,-50,69,6,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-9#Position:547,0,0",
    "2D^Range^Characters,1,2,-50,84,13,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:59#Skew Axis:65#Anchor Point:0,37,23#Rotation:-21#Tracking Type:2#Tracking:-16",
    "2D^Range^Characters,1,2,-50,96,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Anchor Point:428,-71,0#Skew:4#Skew Axis:36#Position:0,-599,0#Blur:8,0",
    "2D^Range^Characters,1,2,-90,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Position:0,-172,0",
    "2D^Range^Characters,1,2,-69,46,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-113,84,0#Rotation:-133#Position:68,0,-408",
    "3D^Range^Words,1,2,-50,95,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:485,0,0#Tracking Type:3#Tracking:-48#Blur:6,0",
    "2D^Range^Characters,1,2,-85,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,241,0#Skew:0#Skew Axis:51#Rotation:0",
    "3D^Range^Characters,1,2,-57,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:9#Skew:12#Skew Axis:18#Position:56,0,576#Rotation:115",
    "3D^Range^Lines,1,2,-83,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,124,0#Rotation:158#Skew:0#Skew Axis:52",
    "2D^Range^Characters,1,2,-44,65,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:166#Position:64,0,-584#Anchor Point:346,0,0",
    "2D^Range^Lines,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:502,0,0#Rotation:157",
    "2D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:91,-63,-434#Blur:26,0#Skew:0#Skew Axis:0#Rotation:100#Tracking Type:3#Tracking:-14",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:25#Rotation:0",
    "3D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:1#Tracking:43#Position:-236,-516,-496#Rotation:61#Blur:0,29#Skew:0#Skew Axis:64",
    "3D^Range^Words,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:25#Rotation:-46#Rotation X:-10#Skew:12#Skew Axis:64#Position:-539,0,0#Blur:27,0",
    "2D^Range^Characters,1,2,-50,83,13,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,-346,0#Blur:0,8#Skew:0#Skew Axis:47#Tracking Type:2#Tracking:0#Position:0,0,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Position:-571,0,-551#Rotation X:75#Blur:10,0#Tracking Type:3#Tracking:-36#Skew:36#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:25#Skew Axis:53#Position:394,-594,0#Blur:0,12#Anchor Point:0,74,474",
    "2D^Range^Characters,1,2,-75,76,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-200,20,480#Blur:0,9#Skew:0#Skew Axis:64#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:168#Position:122,0,0#Tracking Type:2#Tracking:26#Blur:0,26#Skew:8#Skew Axis:18",
    "2D^Range^Words,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0",
    "2D^Range^Words,1,2,-87,79,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:57#Skew Axis:77#Blur:0,1#Tracking Type:2#Tracking:-55",
    "3D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:0#Skew Axis:0#Rotation Y:0#Rotation X:-5#Rotation:118#Blur:0,18",
    "2D^Range^Lines,1,2,-50,83,2,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Anchor Point:150,0,136#Skew:63#Skew Axis:0",
    "2D^Range^Characters,1,2,-94,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:7#Skew Axis:20#Blur:16,0#Anchor Point:0,44,-330",
    "3D^Range^Characters,1,2,-97,98,3,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:-30#Blur:0,20#Rotation X:0#Skew:0#Skew Axis:48#Rotation:-9#Rotation Y:0",
    "3D^Range^Characters,1,2,-92,85,5,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:1#Tracking:37#Anchor Point:0,0,-322#Skew:0#Skew Axis:65#Blur:0,24#Rotation Y:0",
    "3D^Range^Characters,1,2,-50,98,2,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:0,30#Rotation X:0#Rotation Y:0#Position:0,-330,0#Skew:32#Skew Axis:0#Anchor Point:0,0,-486",
    "2D^Range^Characters,1,2,-43,66,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:10#Skew Axis:0#Position:0,104,0",
    "3D^Range^Characters,1,2,-50,97,5,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:22#Rotation Y:-8#Position:201,0,0",
    "2D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:23,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0",
    "2D^Range^Lines,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,-373,0#Anchor Point:0,0,0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-49,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:56#Skew Axis:3#Position:505,405,-105",
    "2D^Range^Words,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:53#Skew Axis:63#Blur:26,0",
    "2D^Range^Characters,1,2,-40,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-515,0,-395#Blur:0,26#Skew:10#Skew Axis:19#Tracking Type:3#Tracking:-10",
    "2D^Range^Lines,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:16,5",
    "2D^Range^Characters,1,2,-80,83,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:3,17#Skew:8#Skew Axis:21#Tracking Type:2#Tracking:-42",
    "2D^Range^Characters,1,2,-92,91,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,8#Position:0,0,0",
    "2D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:8,0#Skew:38#Skew Axis:0#Tracking Type:2#Tracking:0#Position:0,681,81",
    "2D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-46#Blur:1,5#Position:-497,0,0",
    "3D^Range^Words,1,2,-61,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:58#Skew Axis:78#Tracking Type:2#Tracking:30#Rotation:33#Position:-230,370,70",
    "3D^Range^Characters,1,2,-36,50,0,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:-187,0,0#Position:0,-661,0#Rotation X:0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,97,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,248,0#Skew:28#Skew Axis:68#Rotation X:77",
    "3D^Range^Characters,1,2,-76,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,0,494#Rotation:-60#Blur:0,29#Tracking Type:2#Tracking:0#Skew:41#Skew Axis:6",
    "2D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0#Position:0,0,0",
    "2D^Range^Lines,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,331,0#Blur:23,0#Tracking Type:2#Tracking:-9",
    "2D^Range^Characters,1,2,-50,77,7,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-427,-646,0",
    "2D^Range^Characters,1,2,-44,92,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:55#Skew Axis:1#Rotation:0#Tracking Type:1#Tracking:0",
    "3D^Range^Characters,1,2,-50,93,7,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:84,0,0#Rotation:-133#Rotation X:154#Skew:46#Skew Axis:4#Position:467,-107,687#Rotation Y:-74",
    "2D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-97#Tracking Type:2#Tracking:18#Skew:0#Skew Axis:51",
    "3D^Range^Lines,1,2,-44,66,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:-280,23,0#Skew:23#Skew Axis:9#Rotation Y:163",
    "2D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-2#Tracking Type:2#Tracking:-41#Position:220,500,0",
    "3D^Range^Words,1,2,-83,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-156#Blur:10,11#Rotation X:-48#Rotation:32",
    "3D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:179#Rotation X:35#Skew:0#Skew Axis:20",
    "3D^Range^Characters,1,2,-78,73,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:0,-408,-151#Skew:0#Skew Axis:33#Position:0,221,0#Blur:0,10",
    "2D^Range^Characters,1,2,-50,67,11,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:0,9#Tracking Type:3#Tracking:18#Position:512,0,-612",
    "2D^Range^Characters,1,2,-81,71,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Blur:0,20#Position:0,334,-34",
    "2D^Range^Words,1,2,-74,91,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:23#Skew Axis:0#Tracking Type:2#Tracking:0#Anchor Point:-20,0,392",
    "2D^Range^Lines,1,2,-93,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:8#Skew Axis:27#Tracking Type:1#Tracking:0#Position:686,-506,0",
    "2D^Range^Words,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,-494,0#Position:-391,111,631#Tracking Type:3#Tracking:31",
    "3D^Range^Words,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:0#Blur:0,0#Position:20,0,0#Skew:22#Skew Axis:9#Rotation Y:159",
    "3D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation Y:78#Blur:0,7#Anchor Point:-173,0,0#Position:0,423,223",
    "3D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:54#Position:508,0,-608#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,70,8,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:64,0,164#Blur:16,0#Skew:0#Skew Axis:0#Rotation:0",
    "2D^Range^Characters,1,2,-92,91,11,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:31#Anchor Point:0,-215,1#Blur:0,10",
    "2D^Range^Characters,1,2,-65,49,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:265,405,0#Tracking Type:1#Tracking:-55#Skew:16#Skew Axis:15",
    "3D^Range^Characters,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:6#Skew Axis:25#Tracking Type:1#Tracking:0#Rotation Y:58#Position:-666,486,0#Rotation X:151",
    "3D^Range^Words,1,2,-81,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,235,478#Rotation Y:43#Rotation X:-139#Skew:0#Skew Axis:52",
    "2D^Range^Characters,1,2,-50,83,7,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-119",
    "2D^Range^Words,1,2,-50,81,5,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:25,20#Position:-66,0,166",
    "3D^Range^Characters,1,2,-50,78,6,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:37#Rotation Y:-118#Rotation X:-102#Rotation:0",
    "2D^Range^Characters,1,2,-50,89,4,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:37",
    "3D^Range^Characters,1,2,-37,63,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0#Rotation Y:-53#Anchor Point:461,490,0",
    "3D^Range^Characters,1,2,-50,79,8,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:-149#Rotation Y:0#Blur:0,19#Anchor Point:0,-63,-349",
    "2D^Range^Characters,1,2,-50,79,7,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:0#Position:-48,0,0#Rotation:0#Skew:0#Skew Axis:10",
    "3D^Range^Characters,1,2,-50,78,15,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:57#Skew Axis:51#Position:0,-340,260#Rotation:-41",
    "2D^Range^Characters,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,333,-33",
    "2D^Range^Lines,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:8#Rotation:69#Tracking Type:3#Tracking:47",
    "2D^Range^Words,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:23#Skew Axis:51#Anchor Point:262,-62,-434#Position:-297,0,397",
    "3D^Range^Words,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:0#Skew Axis:0#Rotation Y:173#Rotation X:0#Blur:0,19#Tracking Type:3#Tracking:-13",
    "2D^Range^Characters,1,2,-57,50,5,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Tracking Type:3#Tracking:0",
    "3D^Range^Words,1,2,-92,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:152#Skew:45#Skew Axis:3#Position:-340,0,-680",
    "2D^Range^Lines,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:24#Skew Axis:60#Anchor Point:431,-74,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Skew:0#Skew Axis:72#Anchor Point:342,0,0#Rotation:-30#Blur:28,0",
    "3D^Range^Characters,1,2,-50,67,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:0#Rotation X:81#Position:0,336,37#Blur:23,0#Rotation Y:0#Rotation:0",
    "2D^Range^Characters,1,2,-99,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-85,0,0#Tracking Type:2#Tracking:55#Rotation:-36#Skew:34#Skew Axis:25#Position:0,609,0",
    "2D^Range^Lines,1,2,-97,99,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:288,-331,0",
    "3D^Range^Characters,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:0#Skew:12#Skew Axis:0#Rotation:-15#Anchor Point:97,412,440",
    "2D^Range^Characters,1,2,-79,74,4,100,-100!@ANIOM-PRESET@!Opacity:0#Position:300,0,0#Tracking Type:3#Tracking:-17#Blur:0,8",
    "3D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:0,-340,260#Skew:0#Skew Axis:6#Tracking Type:2#Tracking:0#Rotation Y:116",
    "2D^Range^Characters,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:77#Position:-161,-121,161#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-50,98,12,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:351,0,0#Rotation:-34#Skew:7#Skew Axis:26#Tracking Type:1#Tracking:-59",
    "3D^Range^Words,1,2,-36,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:82#Skew:6#Skew Axis:0#Rotation Y:-82#Anchor Point:0,42,28",
    "3D^Range^Characters,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:62#Skew Axis:43#Position:340,0,0#Rotation X:-46",
    "2D^Range^Characters,1,2,-66,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:34#Skew Axis:43#Position:280,0,320#Tracking Type:2#Tracking:-49",
    "2D^Range^Characters,1,2,-50,77,2,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:7#Skew Axis:0#Position:0,0,491",
    "3D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:52#Rotation X:-42#Rotation:106#Skew:35#Skew Axis:45",
    "3D^Range^Words,1,2,-51,61,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-126,0,0#Skew:27#Skew Axis:1#Rotation Y:171#Anchor Point:-290,-490,0",
    "3D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:16#Position:271,0,92#Rotation:-44#Rotation X:8#Skew:11#Skew Axis:72",
    "3D^Range^Words,1,2,-37,50,7,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,-403#Blur:18,0#Skew:0#Skew Axis:61",
    "3D^Range^Lines,1,2,-46,50,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:40#Anchor Point:0,0,0#Position:0,0,-654#Blur:21,6#Skew:0#Skew Axis:38",
    "2D^Range^Characters,1,2,-50,76,15,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:26",
    "2D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-118,489,318#Skew:17#Skew Axis:17#Position:-585,0,-345",
    "3D^Range^Characters,1,2,-98,89,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:54#Rotation X:-162#Rotation:146",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:85",
    "3D^Range^Characters,1,2,-91,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:156#Position:0,0,0#Tracking Type:1#Tracking:59#Rotation X:-45#Rotation Y:153",
    "2D^Range^Characters,1,2,-50,72,6,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:300,0,0",
    "3D^Range^Lines,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Blur:0,0#Position:-14,0,0#Skew:21#Skew Axis:8#Rotation Y:0",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:0#Rotation:0",
    "2D^Range^Words,1,2,-89,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:100,414,443#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-78,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,571,0#Rotation:34#Skew:0#Skew Axis:52",
    "2D^Range^Words,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:2#Tracking:-44#Anchor Point:0,491,0",
    "3D^Range^Characters,1,2,-61,50,1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-481,0#Rotation:152#Tracking Type:1#Tracking:57",
    "2D^Range^Characters,1,2,-50,78,10,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-17#Position:315,0,0#Skew:49#Skew Axis:34",
    "2D^Range^Words,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-393,0#Rotation:-28#Skew:5#Skew Axis:21#Tracking Type:1#Tracking:-57#Blur:0,30",
    "3D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:-89#Position:85,0,0",
    "3D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-36#Rotation:-158#Anchor Point:261,0,-203#Position:0,-605,0#Rotation Y:-6#Blur:15,0",
    "2D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,-360,0",
    "2D^Range^Characters,1,2,-43,68,5,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:-655,695,455#Blur:0,8#Tracking Type:2#Tracking:-17",
    "2D^Range^Characters,1,2,-93,78,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-13#Rotation:33#Anchor Point:0,198,484#Blur:9,10",
    "3D^Range^Characters,1,2,-38,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-39#Rotation X:125",
    "3D^Range^Characters,1,2,-50,87,11,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-37#Position:-293,0,333#Blur:0,0",
    "2D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,165#Rotation:0#Skew:28#Skew Axis:51#Tracking Type:1#Tracking:-57#Anchor Point:0,-218,3",
    "2D^Range^Words,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:365,0,0#Rotation:62#Blur:0,29",
    "2D^Range^Lines,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:25#Skew Axis:60#Position:0,185,0",
    "2D^Range^Words,1,2,-76,71,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-680,0,0#Rotation:-180#Blur:0,19#Skew:16#Skew Axis:71",
    "2D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:0#Skew Axis:35#Rotation:131",
    "3D^Range^Lines,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:47#Position:-463,524,0#Rotation X:-42",
    "3D^Range^Characters,1,2,-50,99,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:-127#Position:-546,0,526#Rotation X:-68#Tracking Type:3#Tracking:0",
    "3D^Range^Lines,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,65,0#Skew:49#Skew Axis:8#Position:151,591,0#Blur:25,20#Rotation X:170",
    "2D^Range^Characters,1,2,-41,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:120#Blur:0,18#Tracking Type:1#Tracking:0#Position:297,0,0",
    "3D^Range^Characters,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Blur:1,26#Tracking Type:2#Tracking:48#Rotation Y:103",
    "3D^Range^Characters,1,2,-39,39,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:506,0,0#Rotation Y:0",
    "2D^Range^Characters,1,2,-81,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:7#Skew:29#Skew Axis:23",
    "2D^Range^Characters,1,2,-97,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:39#Position:-312,0,0",
    "3D^Range^Characters,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-13",
    "2D^Range^Words,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:36#Position:0,-426,526#Skew:13#Skew Axis:26#Rotation:0#Blur:18,0",
    "3D^Range^Characters,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:0,21#Rotation X:26#Position:0,341,-42",
    "3D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:-311,325,440#Position:0,0,0#Rotation X:0#Skew:63#Skew Axis:0",
    "2D^Range^Characters,1,2,-62,43,4,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:-96,0,0#Tracking Type:1#Tracking:-60#Blur:8,0",
    "2D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:9#Skew Axis:70#Position:512,0,0",
    "3D^Range^Characters,1,2,-75,88,14,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,357,57#Rotation Y:0#Tracking Type:2#Tracking:60#Rotation X:0#Anchor Point:0,105,0",
    "2D^Range^Characters,1,2,-50,79,6,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:77#Tracking Type:2#Tracking:26",
    "3D^Range^Words,1,2,-41,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:0#Rotation:0#Skew:0#Skew Axis:45#Position:0,-594,-234#Tracking Type:3#Tracking:-27",
    "2D^Range^Characters,1,2,-49,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:22,0",
    "3D^Range^Words,1,2,-96,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-474,0,0#Skew:42#Skew Axis:39#Position:0,-363,283#Rotation:-57#Tracking Type:2#Tracking:-40",
    "3D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:27#Position:0,186,0#Tracking Type:2#Tracking:-35#Skew:63#Skew Axis:9",
    "2D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-18#Position:294,0,114#Rotation:-50",
    "3D^Range^Characters,1,2,-89,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:2#Tracking:25#Rotation X:0#Anchor Point:0,0,0#Rotation Y:-10#Skew:0#Skew Axis:0#Rotation:0",
    "2D^Range^Characters,1,2,-50,90,11,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:22,8#Anchor Point:0,-42,0#Position:198,478,458#Rotation:-51#Skew:62#Skew Axis:38",
    "2D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:24#Skew Axis:10#Blur:20,5",
    "3D^Range^Words,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:24#Position:0,-532,-512#Rotation X:65#Tracking Type:3#Tracking:-38#Skew:38#Skew Axis:20",
    "2D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Anchor Point:103,74,446#Position:-564,0,0",
    "2D^Range^Characters,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,10#Position:0,-93,493#Skew:62#Skew Axis:0",
    "2D^Range^Words,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:22,0#Tracking Type:3#Tracking:-39#Skew:0#Skew Axis:0#Anchor Point:0,-225,0",
    "3D^Range^Words,1,2,-66,54,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-8#Position:0,-350,270#Tracking Type:2#Tracking:-28#Rotation X:116#Rotation:100#Anchor Point:179,0,0",
    "2D^Range^Characters,1,2,-37,58,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,-239,-353#Blur:19,0#Skew:33#Skew Axis:43#Position:275,0,-315",
    "2D^Range^Words,1,2,-50,100,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Position:0,104,-504",
    "2D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,-359,0#Rotation:0#Skew:0#Skew Axis:0#Anchor Point:0,49,0",
    "3D^Range^Words,1,2,-97,81,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Blur:20,5#Skew:0#Skew Axis:36#Rotation Y:-39",
    "2D^Range^Characters,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:21,5#Skew:11#Skew Axis:24#Tracking Type:2#Tracking:-44#Rotation:0",
    "2D^Range^Characters,1,2,-98,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-35#Position:304,0,0#Skew:0#Skew Axis:32",
    "3D^Range^Characters,1,2,-51,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:5#Anchor Point:0,-360,-346#Rotation:58#Blur:0,28#Skew:14#Skew Axis:14#Position:564,-24,244",
    "3D^Range^Words,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:1#Tracking:0#Rotation:-25#Blur:0,17#Rotation X:0",
    "2D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Blur:21,5#Skew:11#Skew Axis:23#Position:0,255,-656",
    "2D^Range^Lines,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:37#Skew:33#Skew Axis:42#Anchor Point:192,0,64#Rotation:-44",
    "3D^Range^Characters,1,2,-46,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:127#Skew:62#Skew Axis:44#Position:-344,0,0",
    "3D^Range^Characters,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-306,0,0#Position:0,-349,0#Rotation Y:177#Rotation X:162",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:493,0,0#Skew:19#Skew Axis:6#Rotation:-8#Tracking Type:1#Tracking:3",
    "3D^Range^Characters,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:0,343,-263#Skew:0#Skew Axis:6#Tracking Type:2#Tracking:0#Rotation Y:116",
    "3D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Rotation:0#Skew:17#Skew Axis:79#Position:0,42,-142",
    "3D^Range^Characters,1,2,-90,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:61#Skew Axis:59#Rotation Y:-84#Anchor Point:0,48,0#Position:207,-487,467#Tracking Type:2#Tracking:-17",
    "3D^Range^Characters,1,2,-54,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-330,0#Rotation X:172#Rotation Y:-116#Skew:55#Skew Axis:1",
    "2D^Range^Characters,1,2,-51,61,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:35#Position:-310,0,-350#Anchor Point:179,0,50",
    "2D^Range^Characters,1,2,-98,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:574,-34,-254",
    "3D^Range^Characters,1,2,-89,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:69#Blur:25,0#Rotation X:-54#Tracking Type:2#Tracking:57#Rotation:-41#Position:558,0,0",
    "3D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-38#Skew:34#Skew Axis:38#Position:-283,0,0",
    "3D^Range^Words,1,2,-83,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:314,0,134#Rotation X:-55#Rotation Y:-19#Rotation:40",
    "2D^Range^Characters,1,2,-56,50,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,0,0#Rotation:-102#Anchor Point:0,0,0#Skew:0#Skew Axis:75#Tracking Type:2#Tracking:-20",
    "2D^Range^Characters,1,2,-50,96,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:0,402,0#Rotation:0#Skew:53#Skew Axis:0",
    "2D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:3#Tracking:-45#Anchor Point:-5,0,77#Position:288,0,0",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-124#Position:-2,0,0#Tracking Type:1#Tracking:-48#Skew:48#Skew Axis:0#Anchor Point:0,-359,0",
    "2D^Range^Lines,1,2,-60,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:22,402,-322#Tracking Type:3#Tracking:37",
    "2D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Position:469,0,0#Skew:0#Skew Axis:47#Tracking Type:1#Tracking:10#Blur:24,0",
    "3D^Range^Characters,1,2,-50,76,8,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:-58#Blur:0,0#Rotation X:-82",
    "3D^Range^Lines,1,2,-81,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-83#Position:0,345,-265#Rotation X:71",
    "3D^Range^Characters,1,2,-93,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation Y:159#Blur:0,10#Rotation X:-143#Tracking Type:3#Tracking:-59",
    "3D^Range^Characters,1,2,-72,72,13,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Anchor Point:0,-158,0#Rotation Y:-41#Blur:0,21",
    "3D^Range^Characters,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:49#Skew Axis:0#Rotation Y:19#Rotation:163#Anchor Point:0,0,0#Tracking Type:2#Tracking:49#Blur:15,0#Rotation X:-173",
    "3D^Range^Characters,1,2,-71,76,11,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:82#Rotation Y:46#Skew:0#Skew Axis:80#Position:-538,0,0",
    "2D^Range^Lines,1,2,-94,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:-180,0,0#Skew:23#Skew Axis:59#Position:0,-172,0",
    "3D^Range^Characters,1,2,-94,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-165#Rotation:144#Blur:19,22",
    "2D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:-56#Skew:0#Skew Axis:24",
    "2D^Range^Characters,1,2,-56,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:46#Blur:0,17#Skew:38#Skew Axis:42#Position:-316,0,356#Anchor Point:-183,0,-54",
    "2D^Range^Words,1,2,-46,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:136",
    "3D^Range^Characters,1,2,-50,92,3,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:61#Tracking Type:2#Tracking:0#Rotation:-145#Blur:0,0#Position:266,0,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-27#Blur:21,6#Skew:11#Skew Axis:21#Rotation X:77#Anchor Point:0,-228,171",
    "2D^Range^Words,1,2,-37,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-28#Anchor Point:0,-360,0",
    "3D^Range^Characters,1,2,-50,70,10,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-45#Position:0,204,0#Anchor Point:-431,74,-103",
    "2D^Range^Characters,1,2,-70,61,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:558,0,0#Anchor Point:456,-327,0",
    "2D^Range^Words,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:2,5#Anchor Point:0,-366,0",
    "2D^Range^Lines,1,2,-74,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:16#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,87,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,0,140#Rotation:0#Anchor Point:300,0,0#Tracking Type:2#Tracking:40#Rotation Y:167",
    "3D^Range^Words,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:119#Tracking Type:1#Tracking:-14#Rotation Y:16#Skew:14#Skew Axis:66",
    "3D^Range^Words,1,2,-82,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Rotation X:0#Skew:63#Skew Axis:69#Rotation Y:0#Tracking Type:1#Tracking:7#Anchor Point:0,-362,191",
    "3D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:186,-286,0#Rotation Y:0#Position:0,340,-260#Tracking Type:2#Tracking:-46#Blur:0,17",
    "3D^Range^Characters,1,2,-98,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-176#Blur:0,18#Tracking Type:2#Tracking:-23#Rotation X:-9#Skew:11#Skew Axis:72",
    "2D^Range^Characters,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:48#Rotation:-92",
    "2D^Range^Characters,1,2,-76,95,12,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Skew:40#Skew Axis:0#Tracking Type:3#Tracking:-10#Position:0,539,-299",
    "2D^Range^Characters,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:-82,0,0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-77,74,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:57#Anchor Point:0,219,-162",
    "3D^Range^Words,1,2,-58,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-178#Blur:0,18#Tracking Type:1#Tracking:-28#Rotation Y:-121",
    "2D^Range^Characters,1,2,-63,89,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:-106,0,0#Skew:0#Skew Axis:51",
    "2D^Range^Characters,1,2,-67,52,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:1#Tracking:0#Position:491,0,-591#Skew:7#Skew Axis:20#Blur:16,0",
    "3D^Range^Characters,1,2,-51,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:10,10#Skew:56#Skew Axis:3#Rotation:88#Anchor Point:-316,0,-431",
    "3D^Range^Characters,1,2,-87,85,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:37#Skew:34#Skew Axis:0#Rotation:-81#Rotation Y:0",
    "3D^Range^Characters,1,2,-73,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-685,505,0#Rotation X:-42#Rotation Y:-132#Skew:38#Skew Axis:41",
    "2D^Range^Characters,1,2,-96,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:38#Position:0,379,0#Skew:38#Skew Axis:14",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,45#Tracking Type:2#Tracking:0#Anchor Point:0,0,332",
    "2D^Range^Lines,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Skew:22#Skew Axis:11#Position:497,0,0",
    "3D^Range^Characters,1,2,-96,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:157#Tracking Type:1#Tracking:49#Anchor Point:-59,0,-173#Rotation Y:6",
    "3D^Range^Words,1,2,-86,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:34#Skew Axis:38#Position:276,0,0#Rotation Y:174#Rotation:138",
    "3D^Range^Characters,1,2,-46,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:-161#Position:426,0,0",
    "3D^Range^Characters,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-24#Rotation:-67",
    "3D^Range^Words,1,2,-51,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:152#Tracking Type:2#Tracking:-29#Skew:0#Skew Axis:0#Rotation Y:-122#Position:-433,493,0",
    "2D^Range^Characters,1,2,-87,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-38#Position:-293,0,-453#Blur:0,15#Rotation:22",
    "3D^Range^Characters,1,2,-50,70,13,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-445,0,0#Rotation X:9#Rotation Y:32",
    "2D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-383,0#Skew:62#Skew Axis:60",
    "3D^Range^Characters,1,2,-100,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:7#Skew Axis:26#Tracking Type:2#Tracking:59#Anchor Point:0,-218,0#Rotation X:0#Rotation Y:68#Position:405,-605,0",
    "2D^Range^Lines,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:-12,0,0#Tracking Type:2#Tracking:0#Rotation:0#Position:267,-407,0",
    "3D^Range^Characters,1,2,-86,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:38#Rotation X:15#Skew:17#Skew Axis:72#Position:330,30,0",
    "3D^Range^Characters,1,2,-50,71,14,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:61#Rotation Y:60#Position:-672,11,0",
    "3D^Range^Words,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,-212,0#Rotation Y:35",
    "2D^Range^Characters,1,2,-66,50,14,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,364,0",
    "2D^Range^Lines,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Anchor Point:23,0,-95#Skew:0#Skew Axis:0#Blur:26,0",
    "2D^Range^Words,1,2,-72,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,-291,571#Skew:23#Skew Axis:0#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:0#Tracking Type:1#Tracking:-13#Blur:0,15#Skew:33#Skew Axis:50#Rotation Y:0#Anchor Point:360,0,0",
    "3D^Range^Words,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:76#Tracking Type:1#Tracking:36#Skew:14#Skew Axis:12#Position:-557,0,-537#Anchor Point:198,483,141",
    "3D^Range^Characters,1,2,-68,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:24#Skew:0#Skew Axis:0#Position:321,341,0#Blur:0,11",
    "3D^Range^Lines,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:54#Rotation X:-152#Rotation Y:20#Rotation:-41#Skew:58#Skew Axis:71",
    "3D^Range^Characters,1,2,-50,98,14,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:46#Rotation Y:-134#Skew:12#Skew Axis:25#Blur:0,17#Position:0,332,0",
    "3D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:0#Anchor Point:-281,0,0#Rotation:0#Skew:27#Skew Axis:49",
    "3D^Range^Words,1,2,-69,72,4,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:19#Rotation Y:-76#Rotation X:58#Position:-527,0,0",
    "3D^Range^Words,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Scale:0,0",
    "2D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:98#Skew:32#Skew Axis:55#Position:-260,400,100",
    "3D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Blur:1,6",
    "2D^Range^Characters,1,2,-62,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,12#Tracking Type:3#Tracking:36#Position:-278,0,-318",
    "2D^Range^Characters,1,2,-38,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:9#Skew Axis:19#Position:0,580,0#Rotation:-159",
    "2D^Range^Characters,1,2,-96,85,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0",
    "2D^Range^Words,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Position:578,0,-338#Tracking Type:1#Tracking:-5",
    "3D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:174#Position:-56,0,0#Rotation:66#Tracking Type:3#Tracking:29#Skew:54#Skew Axis:73#Blur:6,6#Anchor Point:0,-240,-483#Rotation Y:-45",
    "2D^Range^Characters,1,2,-65,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Anchor Point:-463,335,0",
    "2D^Range^Words,1,2,-50,99,9,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:30#Rotation:-22#Skew:5#Skew Axis:30",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:0#Skew:0#Skew Axis:0#Tracking Type:2#Tracking:0#Position:219,-39,499",
    "3D^Range^Words,1,2,-72,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:15#Skew Axis:43#Rotation:132#Anchor Point:-266,409,0#Blur:0,25#Position:692,0,152",
    "3D^Range^Characters,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:176#Position:0,126,-46#Rotation Y:94#Blur:29,11#Anchor Point:-133,0,0",
    "2D^Range^Words,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:62#Skew Axis:0#Tracking Type:2#Tracking:27",
    "2D^Range^Characters,1,2,-66,50,5,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-46#Skew:0#Skew Axis:77",
    "2D^Range^Lines,1,2,-84,86,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:26,6#Anchor Point:0,-236,0#Skew:16#Skew Axis:64#Position:0,-100,500",
    "2D^Range^Words,1,2,-94,72,1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:69#Skew Axis:27#Position:-626,126,-166#Tracking Type:2#Tracking:-37#Blur:30,0",
    "2D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-98,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:59#Skew Axis:57#Anchor Point:0,37,23#Tracking Type:3#Tracking:-38",
    "3D^Range^Characters,1,2,-80,95,15,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:0#Tracking Type:2#Tracking:0#Rotation Y:148#Position:0,595,-514#Anchor Point:0,218,203",
    "3D^Range^Characters,1,2,-76,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:2#Tracking:-55#Anchor Point:0,-283,0#Rotation X:158#Rotation:71",
    "2D^Range^Words,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-62#Blur:0,9#Skew:40#Skew Axis:43#Position:119,0,-99",
    "2D^Range^Words,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:9,10#Skew:0#Skew Axis:0#Rotation:-87",
    "2D^Range^Characters,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:0#Skew Axis:0#Tracking Type:2#Tracking:0#Position:-692,0,0",
    "3D^Range^Characters,1,2,-48,100,11,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,-336,0#Rotation X:0#Skew:0#Skew Axis:0",
    "2D^Range^Words,1,2,-66,50,12,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-38#Position:299,0,0",
    "3D^Range^Words,1,2,-73,78,9,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:39#Rotation X:20#Anchor Point:0,228,-14#Rotation Y:95#Position:-89,0,0",
    "2D^Range^Words,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-16#Position:310,0,130",
    "2D^Range^Characters,1,2,-50,74,3,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:1#Tracking:-59#Anchor Point:0,220,0#Blur:5,0",
    "2D^Range^Characters,1,2,-54,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-179,-365,0#Position:-120,0,580",
    "3D^Range^Characters,1,2,-84,96,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:82#Rotation X:64#Skew:55#Skew Axis:49#Position:420,680,140#Tracking Type:1#Tracking:-58",
    "3D^Range^Words,1,2,-44,57,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:22#Skew Axis:0#Position:83,0,0#Rotation:-34#Rotation X:86",
    "3D^Range^Characters,1,2,-76,77,1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation X:0#Position:0,0,0",
    "2D^Range^Lines,1,2,-96,76,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,0,426#Tracking Type:2#Tracking:-57#Position:346,0,-386",
    "3D^Range^Words,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-121#Skew:7#Skew Axis:20#Blur:7,0#Rotation Y:137#Rotation:-34",
    "3D^Range^Words,1,2,-97,83,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,409,152#Skew:41#Skew Axis:6#Tracking Type:1#Tracking:-4",
    "2D^Range^Words,1,2,-93,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:-47#Position:467,-47,0",
    "2D^Range^Lines,1,2,-50,99,4,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:289,0,329#Blur:0,21#Anchor Point:499,0,-70#Rotation:0#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-50,95,10,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:11,6#Rotation X:57#Position:131,0,0#Rotation:147",
    "3D^Range^Words,1,2,-54,60,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:39#Rotation X:0#Anchor Point:265,-64,0#Skew:0#Skew Axis:0",
    "3D^Range^Words,1,2,-87,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:-491,0,0#Tracking Type:3#Tracking:-31#Blur:29,10#Rotation Y:5#Rotation X:170",
    "2D^Range^Words,1,2,-84,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:294,0,0#Rotation:88",
    "3D^Range^Characters,1,2,-87,73,8,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-163#Rotation:148",
    "2D^Range^Characters,1,2,-61,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:-57#Blur:0,10#Position:0,-580,0#Skew:30#Skew Axis:0",
    "2D^Range^Words,1,2,-65,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:124#Position:0,252,-532#Skew:47#Skew Axis:5#Anchor Point:94,409,-437",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0",
    "3D^Range^Words,1,2,-50,90,5,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation Y:0#Skew:49#Skew Axis:0#Rotation:-38#Rotation X:0",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:16,0#Position:-309,0,349#Anchor Point:-178,0,0",
    "3D^Range^Characters,1,2,-50,90,11,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:58#Rotation Y:-118#Rotation X:103#Rotation:88",
    "2D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,534,0#Anchor Point:0,439,0",
    "2D^Range^Words,1,2,-99,91,5,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:0,91,334#Skew:67#Skew Axis:35",
    "3D^Range^Characters,1,2,-87,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-36#Position:-277,0,317#Anchor Point:-241,0,0#Blur:0,7",
    "2D^Range^Words,1,2,-77,74,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-29#Skew:55#Skew Axis:47",
    "2D^Range^Characters,1,2,-50,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:12,0#Tracking Type:3#Tracking:35#Position:-314,0,354",
    "3D^Range^Characters,1,2,-39,50,14,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:-43#Rotation:27#Skew:53#Skew Axis:79#Tracking Type:1#Tracking:0",
    "2D^Range^Characters,1,2,-68,86,15,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:31#Skew Axis:54#Tracking Type:1#Tracking:-59#Position:0,-329,-29",
    "3D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-162,-602,0#Rotation Y:-3#Skew:21#Skew Axis:63#Rotation:-13",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:5#Skew Axis:24#Tracking Type:1#Tracking:58",
    "2D^Range^Characters,1,2,-39,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:63#Skew Axis:39#Position:0,-216,0#Rotation:102",
    "2D^Range^Characters,1,2,-50,72,10,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:2#Tracking:19#Position:-300,0,399#Blur:0,26#Skew:0#Skew Axis:0#Anchor Point:0,-50,0",
    "3D^Range^Words,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:-35#Position:-274,0,0#Rotation Y:-173#Rotation:-137#Rotation X:158#Skew:47#Skew Axis:6#Anchor Point:96,-67,-439#Blur:0,4",
    "3D^Range^Words,1,2,-50,83,3,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:57#Skew Axis:70#Anchor Point:-363,0,0",
    "3D^Range^Lines,1,2,-50,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:22#Skew Axis:8#Rotation Y:160#Anchor Point:0,-73,-473#Position:0,-642,-502",
    "3D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,75,0#Rotation X:45#Rotation Y:135",
    "2D^Range^Characters,1,2,-42,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:58#Skew Axis:77#Rotation:84#Position:0,-567,-267#Blur:0,9#Tracking Type:1#Tracking:-19",
    "3D^Range^Characters,1,2,-48,50,5,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:0#Skew Axis:53#Rotation X:0#Position:-21,401,-321#Tracking Type:2#Tracking:28#Rotation:-18",
    "2D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:20,5#Skew:9#Skew Axis:19#Anchor Point:0,-217,3",
    "2D^Range^Words,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:3#Skew Axis:0",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:58#Rotation X:0#Rotation Y:0#Anchor Point:0,227,-170#Skew:0#Skew Axis:60",
    "2D^Range^Words,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:33#Skew Axis:56#Blur:0,2",
    "3D^Range^Characters,1,2,-88,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-14,0,-386#Skew:57#Skew Axis:67#Rotation Y:82#Rotation X:-144#Position:0,-579,499",
    "3D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:6#Position:0,0,-404#Rotation X:-166#Tracking Type:2#Tracking:19#Rotation Y:78",
    "3D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:71#Rotation X:0#Tracking Type:1#Tracking:0",
    "2D^Range^Characters,1,2,-83,95,0,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Anchor Point:185,0,0#Tracking Type:2#Tracking:0#Rotation:-82#Position:0,179,-459",
    "3D^Range^Lines,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:75#Rotation X:0#Tracking Type:1#Tracking:29#Blur:1,5",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:76#Position:0,437,237#Anchor Point:0,-126,0#Tracking Type:2#Tracking:-23",
    "2D^Range^Characters,1,2,-50,95,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:0#Skew:27#Skew Axis:49",
    "3D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Anchor Point:-362,476,-133#Rotation X:161#Position:296,0,116#Rotation:-50#Rotation Y:-14",
    "2D^Range^Lines,1,2,-59,42,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:31#Position:329,0,369#Anchor Point:0,-235,0",
    "3D^Range^Characters,1,2,-83,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Anchor Point:-203,0,0#Rotation X:130#Tracking Type:2#Tracking:-20#Blur:0,7",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:47#Skew Axis:31#Anchor Point:91,62,0#Position:0,-177,37#Tracking Type:2#Tracking:-31",
    "2D^Range^Characters,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,19#Position:0,95,-495",
    "3D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:64#Tracking Type:2#Tracking:-23#Rotation Y:0#Rotation X:0#Anchor Point:135,335,320",
    "2D^Range^Characters,1,2,-42,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:302,-2,0#Skew:39#Skew Axis:67",
    "2D^Range^Characters,1,2,-46,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-68,0#Blur:16,22#Position:-565,0,0",
    "2D^Range^Words,1,2,-72,90,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:11,26",
    "3D^Range^Words,1,2,-98,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:-31#Rotation Y:0#Position:0,-352,0",
    "2D^Range^Characters,1,2,-76,90,5,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:151#Skew:53#Skew Axis:78#Position:0,0,-365",
    "3D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation X:0#Tracking Type:3#Tracking:0#Blur:10,5",
    "2D^Range^Characters,1,2,-97,73,8,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-562,0,-602#Anchor Point:359,0,0",
    "3D^Range^Characters,1,2,-57,60,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:17#Rotation:45#Position:0,-204,0#Rotation X:-166",
    "2D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-36#Position:316,0,356",
    "2D^Range^Characters,1,2,-50,78,6,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-44#Skew:11#Skew Axis:24#Anchor Point:156,0,-356",
    "2D^Range^Words,1,2,-50,88,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-44,-1,0#Rotation:-57#Blur:0,28#Tracking Type:1#Tracking:-44#Position:-562,0,0",
    "2D^Range^Words,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,11#Position:397,117,637#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-39,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:54#Skew Axis:42#Blur:27,0",
    "3D^Range^Characters,1,2,-94,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Tracking Type:1#Tracking:-2#Blur:7,5#Rotation X:4#Skew:57#Skew Axis:70#Position:514,0,0",
    "2D^Range^Characters,1,2,-59,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,28#Tracking Type:3#Tracking:49#Rotation:71#Position:477,0,-576",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:6,0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-164#Skew:2#Skew Axis:30#Rotation X:41#Rotation:133#Anchor Point:270,70,0#Blur:22,0#Tracking Type:2#Tracking:28",
    "2D^Range^Words,1,2,-44,50,10,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:20",
    "3D^Range^Characters,1,2,-50,87,6,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:0#Rotation Y:-155#Blur:0,0",
    "3D^Range^Characters,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-46#Skew:50#Skew Axis:2#Rotation Y:-50#Anchor Point:-247,476,333#Rotation X:84",
    "3D^Range^Lines,1,2,-47,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:22#Skew Axis:7#Rotation:14#Anchor Point:0,275,0",
    "3D^Range^Lines,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,27,-247#Rotation Y:-53#Anchor Point:462,333,319#Skew:63#Skew Axis:69",
    "2D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:5#Skew Axis:27#Position:-409,-469,449#Rotation:-49#Tracking Type:2#Tracking:26",
    "3D^Range^Characters,1,2,-62,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:-35#Rotation:19#Rotation Y:163#Skew:49#Skew Axis:33#Position:152,-112,0#Rotation X:175",
    "2D^Range^Lines,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:685,-25,-485#Tracking Type:2#Tracking:17",
    "3D^Range^Characters,1,2,-65,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:20#Skew Axis:0#Blur:0,29#Tracking Type:2#Tracking:-29",
    "3D^Range^Characters,1,2,-35,58,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-103#Position:322,-481,0#Anchor Point:0,-30,-15#Rotation X:70#Rotation Y:162#Skew:49#Skew Axis:29",
    "2D^Range^Words,1,2,-79,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-264,0#Skew:12#Skew Axis:74",
    "2D^Range^Lines,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-560,0,0",
    "3D^Range^Words,1,2,-71,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:54#Skew Axis:0#Position:482,-382,82#Rotation X:-83",
    "3D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:5#Skew Axis:0#Rotation X:12",
    "2D^Range^Characters,1,2,-50,66,12,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:0",
    "3D^Range^Lines,1,2,-55,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:15#Skew Axis:70#Position:-571,0,331#Rotation X:-13#Tracking Type:3#Tracking:34",
    "2D^Range^Characters,1,2,-43,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:0#Rotation:-76",
    "2D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:21,26#Skew:0#Skew Axis:37",
    "3D^Range^Words,1,2,-79,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-85#Position:0,351,-271#Tracking Type:2#Tracking:-38",
    "2D^Range^Characters,1,2,-92,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,570,0#Rotation:-157#Skew:0#Skew Axis:51",
    "2D^Range^Lines,1,2,-83,99,7,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:49#Blur:0,0",
    "3D^Range^Characters,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-447,-247#Tracking Type:2#Tracking:33#Skew:47#Skew Axis:5#Anchor Point:0,-98,0#Rotation Y:-156#Blur:10,11#Rotation:146#Rotation X:48",
    "2D^Range^Lines,1,2,-50,86,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:72#Position:0,420,0#Tracking Type:2#Tracking:38#Blur:8,0",
    "3D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:15,0#Rotation X:-170#Tracking Type:2#Tracking:57",
    "3D^Range^Words,1,2,-39,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:220,-281,0#Rotation X:-46#Anchor Point:-322,0,0#Rotation Y:-111#Tracking Type:2#Tracking:16",
    "2D^Range^Characters,1,2,-50,77,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,224,-381#Skew:0#Skew Axis:0",
    "3D^Range^Lines,1,2,-57,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,0,187#Rotation X:0#Skew:31#Skew Axis:53#Tracking Type:1#Tracking:59#Anchor Point:0,234,-19",
    "2D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:17#Position:-280,0,-100#Rotation:46#Skew:12#Skew Axis:73",
    "2D^Range^Words,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:26#Position:0,556,0#Rotation:0#Tracking Type:2#Tracking:0",
    "3D^Range^Characters,1,2,-50,89,9,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:21#Skew Axis:7#Rotation:12#Anchor Point:-135,-335,0#Rotation X:38",
    "3D^Range^Characters,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:16#Rotation:129#Position:0,72,52",
    "2D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0#Rotation:0#Tracking Type:3#Tracking:17",
    "3D^Range^Characters,1,2,-63,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:2#Tracking:36#Rotation:143#Rotation Y:-164#Skew:50#Skew Axis:34#Position:-158,118,0#Blur:0,0",
    "3D^Range^Words,1,2,-57,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation Y:0#Tracking Type:3#Tracking:-35#Position:-311,0,0#Rotation X:0",
    "3D^Range^Lines,1,2,-50,82,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Blur:8,0#Skew:38#Skew Axis:42#Position:104,0,84#Anchor Point:0,-374,0#Rotation Y:-166",
    "3D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,-212,-197#Tracking Type:3#Tracking:37#Position:0,-26,0",
    "3D^Range^Characters,1,2,-50,75,11,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-212,-273,0#Rotation X:-44#Skew:68#Skew Axis:3#Anchor Point:0,-37,109#Rotation:34#Tracking Type:2#Tracking:33",
    "2D^Range^Lines,1,2,-50,97,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:-165#Position:22,0,0#Skew:0#Skew Axis:0#Tracking Type:3#Tracking:0",
    "2D^Range^Words,1,2,-50,74,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:11#Skew Axis:18#Position:-54,0,0#Blur:29,4",
    "2D^Range^Characters,1,2,-50,71,0,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:48#Skew:13#Skew Axis:65#Position:547,0,-587#Anchor Point:-348,0,0",
    "2D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:2,6#Position:517,0,-617",
    "2D^Range^Lines,1,2,-56,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:61#Skew Axis:65#Position:555,0,0#Blur:27,3",
    "2D^Range^Characters,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:2#Tracking:-56#Rotation:40#Anchor Point:-169,0,241#Position:17,0,0#Skew:22#Skew Axis:8",
    "2D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:53#Skew Axis:78#Tracking Type:1#Tracking:-38#Blur:29,11",
    "2D^Range^Characters,1,2,-50,90,7,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:5#Skew Axis:0#Anchor Point:0,206,192#Tracking Type:2#Tracking:17",
    "2D^Range^Characters,1,2,-50,68,13,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Position:0,0,398#Skew:0#Skew Axis:40",
    "2D^Range^Words,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-44#Skew:59#Skew Axis:72#Position:-530,0,570",
    "2D^Range^Characters,1,2,-50,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:-64#Anchor Point:0,-100,0#Tracking Type:2#Tracking:-54#Skew:35#Skew Axis:39",
    "3D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:59#Skew Axis:0#Tracking Type:2#Tracking:27#Rotation X:0",
    "2D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-112#Skew:0#Skew Axis:63",
    "3D^Range^Lines,1,2,-50,74,10,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-27#Rotation:148#Skew:52#Skew Axis:77",
    "3D^Range^Characters,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:0#Skew:0#Skew Axis:0",
    "2D^Range^Characters,1,2,-98,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:22,21#Tracking Type:2#Tracking:-45#Rotation:-24",
    "2D^Range^Words,1,2,-50,81,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:36#Blur:23,0#Position:-316,156,436",
    "2D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Position:0,386,0#Rotation:0#Anchor Point:0,0,490",
    "2D^Range^Words,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:3#Skew Axis:19#Rotation:33",
    "2D^Range^Words,1,2,-76,93,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-45#Blur:5,5#Anchor Point:0,230,-473",
    "2D^Range^Lines,1,2,-67,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,0#Position:15,0,0#Skew:21#Skew Axis:9#Rotation:14",
    "2D^Range^Words,1,2,-98,80,5,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:25#Skew Axis:61#Position:0,190,0#Rotation:46",
    "3D^Range^Characters,1,2,-51,43,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-38#Position:-303,0,0#Rotation:-145#Rotation Y:165",
    "2D^Range^Lines,1,2,-55,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:16#Position:-305,0,-126",
    "3D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:39#Position:-317,0,-357#Anchor Point:184,0,0#Rotation Y:169#Rotation:133#Rotation X:-153#Skew:46#Skew Axis:4",
    "3D^Range^Words,1,2,-50,89,11,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,99,-499#Skew:0#Skew Axis:28#Rotation Y:-177",
    "3D^Range^Characters,1,2,-50,79,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,590,0#Rotation X:-13#Skew:38#Skew Axis:16",
    "2D^Range^Lines,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Rotation:-152#Skew:0#Skew Axis:49",
    "2D^Range^Characters,1,2,-40,50,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:655,-695,455#Blur:0,8#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:0",
    "3D^Range^Characters,1,2,-50,96,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:30#Skew Axis:52#Position:0,0,-376#Rotation:0",
    "2D^Range^Characters,1,2,-50,76,6,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:15,0",
    "3D^Range^Characters,1,2,-76,70,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Skew:27#Skew Axis:49#Anchor Point:-492,0,-107#Position:0,599,0#Tracking Type:1#Tracking:0#Rotation:-18#Rotation Y:0#Blur:0,7",
    "3D^Range^Characters,1,2,-81,83,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:35#Position:-309,0,349",
    "2D^Range^Characters,1,2,-50,65,15,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:43#Skew Axis:8#Rotation:6#Position:-502,-182,102",
    "3D^Range^Words,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-335,0#Rotation:45#Tracking Type:1#Tracking:49",
    "3D^Range^Characters,1,2,-43,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Skew:60#Skew Axis:0#Tracking Type:2#Tracking:0#Position:-456,516,0#Rotation X:0#Blur:0,29",
    "3D^Range^Characters,1,2,-50,67,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:107#Position:-427,0,527#Rotation Y:-7#Skew:33#Skew Axis:24#Blur:0,11",
    "2D^Range^Characters,1,2,-50,91,5,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:69#Tracking Type:3#Tracking:-46#Anchor Point:0,-400,343",
    "2D^Range^Words,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-18",
    "3D^Range^Characters,1,2,-50,98,2,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-83#Anchor Point:0,402,0#Rotation:47#Position:0,93,0",
    "3D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:48#Rotation Y:138#Rotation:-35#Skew:34#Skew Axis:25",
    "2D^Range^Characters,1,2,-53,43,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-665,245#Anchor Point:275,75,0#Blur:0,17#Skew:23#Skew Axis:57",
    "2D^Range^Characters,1,2,-50,73,5,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation:-69#Tracking Type:3#Tracking:-47#Skew:42#Skew Axis:39#Blur:7,26",
    "3D^Range^Lines,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:156#Tracking Type:1#Tracking:30#Rotation X:-84#Skew:51#Skew Axis:3#Rotation Y:-176",
    "2D^Range^Characters,1,2,-98,80,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation:0#Tracking Type:3#Tracking:42#Position:0,108,0",
    "2D^Range^Words,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:0#Rotation:80#Skew:25#Skew Axis:0#Position:140,0,-240",
    "3D^Range^Words,1,2,-50,98,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-171#Rotation:-83#Skew:68#Skew Axis:26#Rotation X:-32#Position:0,203,-483#Tracking Type:1#Tracking:8",
    "2D^Range^Characters,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-16",
    "2D^Range^Characters,1,2,-50,93,13,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Anchor Point:0,33,0#Rotation:0#Tracking Type:2#Tracking:0",
    "2D^Range^Characters,1,2,-50,73,4,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:12#Skew Axis:18#Position:-58,0,0#Blur:7,0",
    "3D^Range^Words,1,2,-88,90,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:49#Rotation Y:13#Skew:13#Skew Axis:65#Position:551,0,591",
    "3D^Range^Words,1,2,-44,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation Y:-66#Anchor Point:-104,0,0#Tracking Type:2#Tracking:-55#Rotation X:128#Skew:0#Skew Axis:0#Rotation:-112",
    "2D^Range^Words,1,2,-50,78,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:-151,111,631#Anchor Point:0,0,-65",
    "2D^Range^Characters,1,2,-96,96,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:-25#Skew:37#Skew Axis:19#Blur:0,7",
    "2D^Range^Characters,1,2,-87,100,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,586,526#Tracking Type:1#Tracking:0#Rotation:0",
    "2D^Range^Lines,1,2,-52,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:23#Skew Axis:51#Position:0,169,-449#Anchor Point:-342,-256,0",
    "2D^Range^Characters,1,2,-50,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:-172#Skew:0#Skew Axis:10",
    "2D^Range^Characters,1,2,-50,94,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-578,498#Skew:45#Skew Axis:49#Tracking Type:3#Tracking:-37#Blur:26,0",
    "3D^Range^Lines,1,2,-50,75,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-57#Rotation X:-75#Anchor Point:0,-222,0#Blur:5,0#Skew:38#Skew Axis:60#Rotation:-88",
    "2D^Range^Characters,1,2,-50,90,5,100,-100!@ANIOM-PRESET@!Scale:100,0#Rotation:29#Skew:53#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:470,0,0#Rotation:0",
    "3D^Range^Characters,1,2,-66,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-109,0,-89#Anchor Point:0,378,0#Rotation Y:167#Rotation:108#Rotation X:-128#Skew:36#Skew Axis:46",
    "3D^Range^Words,1,2,-38,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,-387,0#Anchor Point:-477,0,-91#Blur:22,0#Tracking Type:3#Tracking:0",
    "3D^Range^Characters,1,2,-36,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Rotation X:-110#Rotation:0#Tracking Type:3#Tracking:26#Rotation Y:0",
    "3D^Range^Words,1,2,-50,86,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-3#Tracking Type:3#Tracking:47#Position:-30,0,0#Skew:23#Skew Axis:10",
    "3D^Range^Characters,1,2,-59,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-88#Skew:28#Skew Axis:50#Position:221,361,-61#Blur:0,0#Rotation X:82",
    "2D^Range^Characters,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:367,0,-24#Position:694,-514,-494#Rotation:55#Blur:0,28",
    "2D^Range^Words,1,2,-99,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-395,75,0#Rotation:89#Blur:4,0#Tracking Type:2#Tracking:9",
    "2D^Range^Characters,1,2,-77,82,9,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-26",
    "3D^Range^Characters,1,2,-50,69,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:36#Tracking Type:3#Tracking:-17#Rotation Y:72#Blur:0,8",
    "3D^Range^Characters,1,2,-50,84,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,183,-463#Rotation:-3#Tracking Type:3#Tracking:-47",
    "2D^Range^Characters,1,2,-75,76,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation:126#Anchor Point:0,43,28#Blur:2,27",
    "3D^Range^Characters,1,2,-50,93,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:3#Tracking:-22#Skew:24#Skew Axis:12",
    "2D^Range^Characters,1,2,-50,80,14,100,-100!@ANIOM-PRESET@!Scale:0,100#Tracking Type:3#Tracking:0#Skew:0#Skew Axis:51",
    "2D^Range^Characters,1,2,-50,73,8,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:0,99,-259#Anchor Point:0,-450,0#Rotation:167",
    "2D^Range^Characters,1,2,-50,84,0,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,533,513#Tracking Type:3#Tracking:-38#Skew:38#Skew Axis:0#Rotation:-21",
    "2D^Range^Characters,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Position:0,-595,0#Skew:31#Skew Axis:54#Rotation:24",
    "3D^Range^Lines,1,2,-52,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,58,0#Skew:28#Skew Axis:50#Tracking Type:1#Tracking:-57",
    "3D^Range^Characters,1,2,-97,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:37#Rotation Y:-132#Skew:37#Skew Axis:41#Position:315,0,-355#Anchor Point:182,0,210#Rotation X:53",
    "3D^Range^Words,1,2,-50,70,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation X:150#Rotation Y:-171#Rotation:0#Tracking Type:2#Tracking:-28#Skew:64#Skew Axis:0",
    "3D^Range^Words,1,2,-62,37,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Rotation Y:39#Rotation X:147",
    "3D^Range^Characters,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Position:579,0,0#Tracking Type:2#Tracking:7",
    "3D^Range^Lines,1,2,-98,93,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Tracking Type:3#Tracking:0#Blur:7,0#Rotation X:0#Skew:0#Skew Axis:65",
    "2D^Range^Characters,1,2,-50,84,1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:0#Position:0,0,0",
    "2D^Range^Lines,1,2,-48,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Skew:0#Skew Axis:0#Anchor Point:-458,486,-315#Tracking Type:1#Tracking:55#Blur:0,0",
    "3D^Range^Characters,1,2,-50,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:2#Tracking:0#Position:2,-522,-502#Rotation Y:0#Rotation:-106",
    "2D^Range^Words,1,2,-50,68,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:32#Skew Axis:48#Rotation:-26#Anchor Point:487,0,102#Position:0,-412,0",
    "2D^Range^Characters,1,2,-37,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Blur:0,7#Position:66,0,0#Skew:22#Skew Axis:9",
    "3D^Range^Lines,1,2,-50,89,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:58#Rotation X:46#Blur:0,17",
    "3D^Range^Characters,1,2,-91,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Skew:10#Skew Axis:14#Position:-40,0,-561#Tracking Type:2#Tracking:5",
    "2D^Range^Characters,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:1#Tracking:-38",
    "2D^Range^Words,1,2,-38,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:0#Skew:38#Skew Axis:0#Tracking Type:2#Tracking:0#Position:-684,-444,0",
    "3D^Range^Characters,1,2,-50,87,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-130#Skew:37#Skew Axis:47#Position:-307,0,-347#Anchor Point:176,0,0",
    "2D^Range^Characters,1,2,-76,82,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Blur:0,18#Position:0,-102,0#Rotation:78",
    "3D^Range^Lines,1,2,-70,50,15,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:0,10#Rotation X:-143#Position:386,0,0#Tracking Type:2#Tracking:-9#Rotation Y:-79#Anchor Point:190,475,0#Rotation:-120#Skew:19#Skew Axis:17",
    "2D^Range^Words,1,2,-50,99,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Tracking Type:3#Tracking:-35#Position:306,0,0#Rotation:74",
    "2D^Range^Lines,1,2,-41,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,-365,-285#Anchor Point:0,-218,0#Blur:4,0",
    "3D^Range^Characters,1,2,-50,92,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:81#Rotation Y:45#Skew:26#Skew Axis:61#Position:-396,116,636#Tracking Type:2#Tracking:-42",
    "3D^Range^Characters,1,2,-50,85,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Position:0,388,28#Tracking Type:2#Tracking:45#Rotation X:-69#Anchor Point:99,70,0#Blur:0,19",
    "3D^Range^Characters,1,2,-55,86,14,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:-159#Anchor Point:93,0,0#Position:-490,-390,0#Skew:27#Skew Axis:43",
    "2D^Range^Lines,1,2,-50,66,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Blur:21,6#Skew:12#Skew Axis:25#Tracking Type:3#Tracking:-45#Position:467,367,-67",
    "2D^Range^Characters,1,2,-68,67,1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:56#Skew Axis:75#Rotation:0#Blur:0,0",
    "2D^Range^Characters,1,2,-50,86,9,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:3#Tracking:47",
    "3D^Range^Characters,1,2,-52,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,-357,0#Tracking Type:2#Tracking:14#Skew:61#Skew Axis:0",
    "2D^Range^Characters,1,2,-45,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:-174,0,202#Tracking Type:1#Tracking:19#Skew:54#Skew Axis:0#Position:-483,383,-83",
    "2D^Range^Lines,1,2,-74,95,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Position:0,0,312#Rotation:0#Skew:0#Skew Axis:22#Anchor Point:480,0,95",
    "2D^Range^Words,1,2,-74,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:62#Position:-420,0,-460#Rotation:0",
    "3D^Range^Characters,1,2,-82,86,5,100,-100!@ANIOM-PRESET@!Opacity:0#Position:-81,0,0#Blur:8,0#Tracking Type:2#Tracking:-55#Rotation Y:-147#Rotation X:75#Rotation:-31",
    "3D^Range^Words,1,2,-98,85,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,100#Rotation X:0#Rotation Y:-41#Tracking Type:3#Tracking:-19#Skew:0#Skew Axis:8",
    "3D^Range^Characters,1,2,-50,80,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Skew:0#Skew Axis:0#Tracking Type:1#Tracking:0#Blur:22,0#Rotation:6#Rotation X:0#Position:-43,0,0",
    "2D^Range^Characters,1,2,-98,95,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Skew:57#Skew Axis:0#Blur:16,0#Anchor Point:0,0,-181",
    "2D^Range^Characters,1,2,-98,88,7,100,-100!@ANIOM-PRESET@!Scale:0,100#Anchor Point:0,-337,-280#Position:0,332,0",
    "2D^Range^Characters,1,2,-50,76,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:0,-122,0#Skew:32#Skew Axis:55",
    "3D^Range^Characters,1,2,-61,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:6#Rotation X:0#Blur:0,18#Position:21,0,0",
    "3D^Range^Characters,1,2,-50,79,9,100,-100!@ANIOM-PRESET@!Opacity:0#Tracking Type:1#Tracking:-57#Rotation X:73#Anchor Point:0,-218,-4#Blur:0,19",
    "2D^Range^Characters,1,2,-98,79,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Position:0,194,-474#Rotation:0#Skew:0#Skew Axis:0",
    "3D^Range^Characters,1,2,-50,72,-1,100,-100!@ANIOM-PRESET@!Scale:0,100#Rotation:0#Position:367,0,0#Tracking Type:2#Tracking:0",
    "3D^Range^Lines,1,2,-49,72,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation:171#Position:-46,0,-566#Anchor Point:-226,0,297",
    "3D^Range^Words,1,2,-50,100,-1,100,-100!@ANIOM-PRESET@!Scale:100,0#Skew:0#Skew Axis:0#Rotation Y:-174#Rotation X:0#Blur:0,19",
    "3D^Range^Characters,1,2,-39,50,-1,100,-100!@ANIOM-PRESET@!Scale:0,0#Rotation:179#Rotation Y:-112#Anchor Point:126,326,0#Blur:0,19#Skew:35#Skew Axis:39",
    "2D^Range^Characters,1,2,-50,68,7,100,-100!@ANIOM-PRESET@!Scale:100,0#Position:-6,526,0#Tracking Type:3#Tracking:47#Rotation:12",
    "2D^Range^Lines,1,2,-68,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:-38",
    "3D^Range^Characters,1,2,-63,50,14,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation X:-164#Rotation:149#Skew:52#Skew Axis:68",
    "3D^Range^Words,1,2,-82,91,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Rotation X:-56#Position:296,0,0#Tracking Type:2#Tracking:14#Rotation:0",
    "2D^Range^Characters,1,2,-50,71,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Anchor Point:0,-106,392#Rotation:-126#Position:0,79,0",
    "3D^Range^Characters,1,2,-94,82,3,100,-100!@ANIOM-PRESET@!Opacity:0#Rotation Y:-37#Tracking Type:3#Tracking:-18#Position:325,0,0#Skew:51#Skew Axis:35",
    "2D^Range^Characters,1,2,-50,79,6,100,-100!@ANIOM-PRESET@!Scale:0,100#Blur:0,0#Tracking Type:3#Tracking:0#Rotation:0#Position:0,-136,0",
    "3D^Range^Words,1,2,-50,73,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Tracking Type:3#Tracking:-36#Position:318,0,358#Anchor Point:-184,0,0#Skew:24#Skew Axis:59",
    "3D^Range^Characters,1,2,-73,97,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:100,0#Blur:0,17#Rotation Y:0#Tracking Type:3#Tracking:0",
    "3D^Range^Words,1,2,-50,96,8,100,-100!@ANIOM-PRESET@!Scale:0,0#Blur:6,20#Position:26,0,247",
    "3D^Range^Characters,1,2,-50,89,10,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Tracking Type:2#Tracking:0#Skew:0#Skew Axis:0#Rotation Y:-172#Anchor Point:291,-491,0#Rotation:-28",
    "2D^Range^Characters,1,2,-64,50,-1,100,-100!@ANIOM-PRESET@!Opacity:0#Scale:0,0#Skew:0#Skew Axis:0#Position:565,0,0#Anchor Point:0,0,425#Tracking Type:2#Tracking:0"
];

(function AnimationKitEffectCraft(thisObj) {
    var SCRIPT_NAME = "Animation Kit — EffectCraft";
    var DELIMITER = "!@ANIOM-PRESET@!";

    function parsePreset(line) {
        var parts = line.split(DELIMITER);
        var header = (parts[0] || "").split("^");
        var selectorParts = (header[2] || "").split(",");
        var properties = [];
        var raw = (parts[1] || "").split("#");
        var i, j;
        for (i = 0; i < raw.length; i++) {
            if (!raw[i]) continue;
            var colon = raw[i].indexOf(":");
            if (colon < 0) continue;
            var name = raw[i].substring(0, colon);
            var vals = raw[i].substring(colon + 1).split(",");
            var values = [];
            for (j = 0; j < vals.length; j++) {
                var n = Number(vals[j]);
                values.push(isNaN(n) ? 0 : n);
            }
            properties.push({ name: name, values: values });
        }
        return {
            dimension: (header[0] || "2D").replace(/^\uFEFF/, ""),
            selectorType: header[1] || "Range",
            basedOn: selectorParts[0] || "Characters",
            mode: Number(selectorParts[1] || 1),
            shape: Number(selectorParts[2] || 2),
            easeHigh: Number(selectorParts[3] || 0),
            easeLow: Number(selectorParts[4] || 0),
            randomSeed: Number(selectorParts[5] || -1),
            offsetStart: Number(selectorParts[6] || 100),
            offsetEnd: Number(selectorParts[7] || -100),
            properties: properties
        };
    }

    function basedOnValue(name) {
        if (name === "Characters Excluding Spaces") return 2;
        if (name === "Words") return 3;
        if (name === "Lines") return 4;
        return 1;
    }

    function getProperty(group, matchName, displayName, internalName) {
        var p = null;
        if (!group) return null;
        if (matchName) {
            try { p = group.property(matchName); } catch (e1) {}
        }
        if (!p && displayName) {
            try { p = group.property(displayName); } catch (e2) {}
        }
        if (!p && internalName) {
            try { p = group.property(internalName); } catch (e3) {}
        }
        return p;
    }

    function setMenu(group, matchName, displayName, internalName, value) {
        var p = getProperty(group, matchName, displayName, internalName);
        if (p) {
            try { p.setValue(value); } catch (e) {
                $.writeln("Animation Kit: could not set selector option " + (displayName || internalName) + ": " + e);
            }
        } else {
            $.writeln("Animation Kit: selector option not exposed by this build: " + (displayName || internalName));
        }
    }

    function propSpec(name) {
        var map = {
            "Anchor Point": { match: "ADBE Text Anchor Point 3D", id: "anchorPoint" },
            "Position": { match: "ADBE Text Position 3D", id: "position" },
            "Scale": { match: "ADBE Text Scale 3D", id: "scale" },
            "Skew": { match: "ADBE Text Skew", id: "skew" },
            "Skew Axis": { match: "ADBE Text Skew Axis", id: "skewAxis" },
            "Rotation": { match: "ADBE Text Rotation", id: "zRotation" },
            "Rotation X": { match: "ADBE Text Rotation X", id: "xRotation" },
            "Rotation Y": { match: "ADBE Text Rotation Y", id: "yRotation" },
            "Opacity": { match: "ADBE Text Opacity", id: "opacity" },
            "Tracking": { match: "ADBE Text Tracking Amount", id: "tracking" },
            "Tracking Type": { match: "ADBE Text Track Type", id: "trackingType" },
            "Character Alignment": { match: "ADBE Text Character Change Type", id: "characterAlignment" },
            "Character Range": { match: "ADBE Text Character Range", id: "characterRange" },
            "Character Offset": { match: "ADBE Text Character Offset", id: "characterOffset" },
            "Blur": { match: "ADBE Text Blur", id: "blur" }
        };
        return map[name] || null;
    }

    function enablePerCharacter3D(layer, textProperties) {
        // AE calls this property threeDPerChar. Some EffectCraft builds expose
        // the same setting through Text > More Options, so try both routes.
        try { layer.threeDPerChar = true; } catch (e1) {}
        try {
            var more = getProperty(textProperties, "ADBE Text More Options", "More Options", "moreOptions");
            var perChar = getProperty(more, "perChar3d", "Per-character 3D", "perChar3d");
            if (perChar) perChar.setValue(true);
        } catch (e2) {
            $.writeln("Animation Kit: this build may not expose per-character 3D through scripting: " + e2);
        }
    }

    function propertyValue(item) {
        var v = item.values.slice(0);
        if (item.name === "Anchor Point" || item.name === "Position" || item.name === "Scale") {
            while (v.length < 3) v.push(0);
            return v;
        }
        if (item.name === "Blur") {
            while (v.length < 2) v.push(0);
            return v;
        }
        // Tracking Type, Opacity, Rotation, Skew, etc. are scalar properties.
        return v.length ? v[0] : 0;
    }

    function describePreset(p) {
        var names = [];
        for (var i = 0; i < p.properties.length; i++) names.push(p.properties[i].name);
        return p.dimension + " / " + p.selectorType + " / " + p.basedOn + "\n" + names.join(", ");
    }

    function applyPreset(preset, direction, presetNumber, targetComp, targetLayer, quiet) {
        // Preview mode can target a temporary sample layer without changing the user's selection.
        var comp = targetComp || app.project.activeItem;
        var layer = targetLayer || (comp && comp.selectedLayers && comp.selectedLayers.length ? comp.selectedLayers[0] : null);
        if (!comp || !layer) {
            if (quiet) $.writeln("Animation Kit preview: no composition or text layer was supplied.");
            else alert("Select a text layer in an active composition first.");
            return false;
        }
        var textProperties = getProperty(layer, "ADBE Text Properties", "Text");
        if (!textProperties) {
            if (quiet) $.writeln("Animation Kit preview: the sample layer does not expose text properties.");
            else alert("The selected layer does not expose text properties. Select a native text layer.");
            return false;
        }

        if (!quiet) app.beginUndoGroup(SCRIPT_NAME + " — Apply " + direction);
        try {
            // The source preset pack explicitly marks these presets as 3D-per-character.
            if (preset.dimension === "3D") {
                enablePerCharacter3D(layer, textProperties);
            }

            var animators = getProperty(textProperties, "ADBE Text Animators", "Animators");
            if (!animators) throw new Error("Text Animators group was not found on this EffectCraft build.");
            var animator = animators.addProperty("ADBE Text Animator");
            animator.name = "AK_EC_" + direction + "_" + presetNumber;

            var selectors = getProperty(animator, "ADBE Text Selectors", "Selectors");
            if (!selectors) throw new Error("Text Selectors group was not found on the animator.");
            var selector = selectors.addProperty("ADBE Text Selector");
            var advanced = getProperty(selector, "ADBE Text Range Advanced", "Advanced", "advanced");
            if (!advanced) {
                $.writeln("Animation Kit: advanced selector options are not exposed by this EffectCraft build; using defaults.");
            } else {
                setMenu(advanced, "ADBE Text Range Type2", "Based On", "basedOn", basedOnValue(preset.basedOn));
                setMenu(advanced, "ADBE Text Selector Mode", "Mode", "mode", preset.mode);
                setMenu(advanced, "ADBE Text Range Shape", "Shape", "shape", preset.shape);
                setMenu(advanced, "ADBE Text Levels Max Ease", "Ease High", "easeHigh", preset.easeHigh);
                setMenu(advanced, "ADBE Text Levels Min Ease", "Ease Low", "easeLow", preset.easeLow);
                if (preset.randomSeed >= 0) {
                    setMenu(advanced, "ADBE Text Randomize Order", "Randomize Order", "randomizeOrder", 1);
                    setMenu(advanced, "ADBE Text Random Seed", "Random Seed", "randomSeed", preset.randomSeed);
                } else {
                    setMenu(advanced, "ADBE Text Randomize Order", "Randomize Order", "randomizeOrder", 0);
                }
            }

            // Percentage offset is the selector unit used by this preset pack.
            var offset = getProperty(selector, "ADBE Text Percent Offset", "Offset");
            if (!offset) offset = getProperty(selector, "ADBE Text Index Offset", "Offset");
            if (!offset) throw new Error("Range-selector Offset property was not found.");

            var startTime, endTime;
            var duration = Math.min(1, Math.max(0, layer.outPoint - layer.inPoint));
            if (direction === "IN") {
                startTime = layer.inPoint;
                endTime = startTime + duration;
                offset.setValueAtTime(startTime, -100);
                offset.setValueAtTime(endTime, 100);
            } else {
                endTime = layer.outPoint;
                startTime = endTime - duration;
                offset.setValueAtTime(startTime, 100);
                offset.setValueAtTime(endTime, -100);
            }

            var props = getProperty(animator, "ADBE Text Animator Properties", "Properties");
            if (!props) throw new Error("Animator Properties group was not found.");
            var skipped = [];
            for (var i = 0; i < preset.properties.length; i++) {
                var item = preset.properties[i];
                var spec = propSpec(item.name);
                if (!spec) { skipped.push(item.name); continue; }
                try {
                    var prop;
                    try { prop = props.addProperty(spec.match); }
                    catch (matchNameError) { prop = props.addProperty(spec.id); }
                    prop.setValue(propertyValue(item));
                } catch (propertyError) {
                    skipped.push(item.name + " (" + propertyError + ")");
                }
            }
            if (skipped.length) {
                $.writeln("Animation Kit: skipped unsupported properties: " + skipped.join(", "));
            }
            return true;
        } catch (e) {
            var errorText = "Could not apply preset " + presetNumber + " (" + direction + "): " +
                  (e && e.message ? e.message : e);
            if (quiet) $.writeln("Animation Kit preview: " + errorText);
            else alert(errorText + "\n\nNo exact compatibility test could be run against your local EffectCraft build.");
            return false;
        } finally {
            if (!quiet) app.endUndoGroup();
        }
    }

    function stylePreviewText(layer, comp, previewText) {
        // Keep styling best-effort: defaults still work if a property is not exposed by this build.
        try {
            var textProperties = getProperty(layer, "ADBE Text Properties", "Text");
            var sourceText = getProperty(textProperties, "ADBE Text Document", "Source Text", "sourceText");
            if (sourceText) {
                var doc = sourceText.value;
                doc.text = String(previewText || "Sample Text");
                doc.fontSize = Math.max(42, Math.round(comp.height * 0.105));
                doc.applyFill = true;
                doc.fillColor = [1, 1, 1];
                doc.applyStroke = false;
                try { doc.justification = ParagraphJustification.CENTER_JUSTIFY; } catch (e1) {}
                sourceText.setValue(doc);
            }
        } catch (e2) {
            $.writeln("Animation Kit preview: using the default text styling: " + e2);
        }
        try {
            var transform = getProperty(layer, "ADBE Transform Group", "Transform");
            var position = getProperty(transform, "ADBE Position", "Position", "position");
            if (position) position.setValue([comp.width / 2, comp.height / 2]);
        } catch (e3) {
            $.writeln("Animation Kit preview: could not center sample text: " + e3);
        }
    }

    function buildUI(thisObj) {
        var win = (typeof Panel !== "undefined" && thisObj instanceof Panel) ? thisObj :
            new Window("palette", SCRIPT_NAME, undefined, { resizeable: true });
        win.orientation = "column";
        win.alignChildren = ["fill", "top"];
        win.spacing = 8;
        win.margins = 12;

        win.add("statictext", undefined, "Animation Kit — Text Presets");
        var panel = win.add("panel", undefined, "Text Preset Volume I (" + AK_PRESETS.length + " presets)");
        panel.orientation = "column";
        panel.alignChildren = ["fill", "top"];
        panel.spacing = 7;
        panel.margins = 10;

        var previewTextRow = panel.add("group");
        previewTextRow.add("statictext", undefined, "Preview text:");
        var sampleTextField = previewTextRow.add("edittext", undefined, "Sample Text");
        sampleTextField.characters = 24;

        function createPresetRow(label, defaultValue) {
            var row = panel.add("group");
            row.alignChildren = ["center", "center"];
            row.add("statictext", undefined, label + ":");
            var slider = row.add("slider", undefined, defaultValue, 1, AK_PRESETS.length);
            slider.minvalue = 1;
            slider.maxvalue = AK_PRESETS.length;
            slider.preferredSize.width = 180;
            var field = row.add("edittext", undefined, String(defaultValue));
            field.characters = 5;
            var enabled = row.add("checkbox", undefined, "Enabled");
            enabled.value = true;
            return { row: row, slider: slider, field: field, enabled: enabled };
        }

        var inControls = createPresetRow("IN", 1);
        var outControls = createPresetRow("OUT", 1);

        var inDetails = panel.add("statictext", undefined, "IN: loading preset details…");
        inDetails.alignment = ["fill", "top"];
        inDetails.preferredSize.height = 32;
        var outDetails = panel.add("statictext", undefined, "OUT: loading preset details…");
        outDetails.alignment = ["fill", "top"];
        outDetails.preferredSize.height = 32;

        var status = win.add("statictext", undefined, "Creating live preview…");
        status.alignment = ["fill", "top"];
        status.preferredSize.height = 30;

        var buttons = win.add("group");
        buttons.alignment = ["right", "top"];
        var refreshButton = buttons.add("button", undefined, "Refresh Preview");
        var applyButton = buttons.add("button", undefined, "Apply to Selected Text");
        var closeButton = buttons.add("button", undefined, "Close");

        // Remember the original editing context before opening the dedicated preview comp.
        var launchComp = app.project.activeItem;
        var launchLayer = launchComp && launchComp.selectedLayers && launchComp.selectedLayers.length ? launchComp.selectedLayers[0] : null;
        var previewComp = null;
        var previewLayer = null;
        var previewCreated = false;
        var isRefreshing = false;

        function readNumber(field) {
            var raw = String(field.text).replace(/^\s+|\s+$/g, "");
            if (!/^\d+$/.test(raw)) return null;
            var n = parseInt(raw, 10);
            if (n < 1 || n > AK_PRESETS.length) return null;
            return n;
        }

        function updateDetails() {
            var inN = readNumber(inControls.field);
            var outN = readNumber(outControls.field);
            inDetails.text = inN === null ? "IN: enter a number from 1 to " + AK_PRESETS.length :
                "IN " + inN + ": " + describePreset(parsePreset(AK_PRESETS[inN - 1])).replace(/\n/g, " · ");
            outDetails.text = outN === null ? "OUT: enter a number from 1 to " + AK_PRESETS.length :
                "OUT " + outN + ": " + describePreset(parsePreset(AK_PRESETS[outN - 1])).replace(/\n/g, " · ");
        }

        function getOrCreatePreviewComp() {
            if (previewComp) return previewComp;
            var sourceComp = app.project.activeItem;
            var width = sourceComp && sourceComp.width ? sourceComp.width : 1280;
            var height = sourceComp && sourceComp.height ? sourceComp.height : 720;
            var pixelAspect = sourceComp && sourceComp.pixelAspect ? sourceComp.pixelAspect : 1;
            var frameRate = sourceComp && sourceComp.frameRate ? sourceComp.frameRate : 30;
            previewComp = app.project.items.addComp("AK Live Preview - Animation Kit", width, height, pixelAspect, 3, frameRate);
            try { previewComp.openInViewer(); } catch (e) {}
            previewCreated = true;
            return previewComp;
        }

        function updatePreview() {
            if (isRefreshing) return;
            updateDetails();
            var inN = readNumber(inControls.field);
            var outN = readNumber(outControls.field);
            if ((inControls.enabled.value && inN === null) || (outControls.enabled.value && outN === null)) {
                status.text = "Enter valid preset numbers to update the live preview.";
                return;
            }
            if (!inControls.enabled.value && !outControls.enabled.value) {
                status.text = "Enable IN and/or OUT to preview an animation.";
                return;
            }

            isRefreshing = true;
            var comp = null;
            try {
                app.beginUndoGroup(SCRIPT_NAME + " — Refresh Live Preview");
                comp = getOrCreatePreviewComp();
                // Rebuild only the preview layer. The user's selected layer is never touched.
                while (comp.numLayers > 0) comp.layer(1).remove();
                var previewText = String(sampleTextField.text || "Sample Text");
                previewLayer = comp.layers.addText(previewText);
                previewLayer.name = "AK Live Preview Text";
                previewLayer.inPoint = 0;
                previewLayer.outPoint = 3;
                stylePreviewText(previewLayer, comp, previewText);

                var inOK = true;
                var outOK = true;
                if (inControls.enabled.value) {
                    inOK = applyPreset(parsePreset(AK_PRESETS[inN - 1]), "IN", inN, comp, previewLayer, true);
                }
                if (outControls.enabled.value) {
                    outOK = applyPreset(parsePreset(AK_PRESETS[outN - 1]), "OUT", outN, comp, previewLayer, true);
                }
                try { comp.time = 0; } catch (timeError) {}
                if (inOK && outOK) status.text = "Live preview updated. Play the AK Live Preview composition to see the motion.";
                else status.text = "Preview refreshed with one or more unsupported properties; see the script console.";
                if (!previewCreated) {
                    try { comp.openInViewer(); } catch (openError) {}
                    previewCreated = true;
                }
            } catch (e) {
                status.text = "Live preview error: " + (e && e.message ? e.message : e);
                $.writeln("Animation Kit live preview error: " + (e && e.message ? e.message : e));
            } finally {
                try { app.endUndoGroup(); } catch (endError) {}
                isRefreshing = false;
            }
        }

        function syncEnabledControls(controls) {
            controls.slider.enabled = controls.enabled.value;
            controls.field.enabled = controls.enabled.value;
        }

        function updateFromSlider(controls) {
            var n = Math.max(1, Math.min(AK_PRESETS.length, Math.round(controls.slider.value)));
            controls.field.text = String(n);
            updatePreview();
        }

        function updateFromField(controls) {
            var n = readNumber(controls.field);
            if (n !== null) controls.slider.value = n;
            updatePreview();
        }

        // Both sliders and number fields refresh the preview immediately while editing.
        inControls.slider.onChanging = function () { updateFromSlider(inControls); };
        inControls.slider.onChange = function () { updateFromSlider(inControls); };
        outControls.slider.onChanging = function () { updateFromSlider(outControls); };
        outControls.slider.onChange = function () { updateFromSlider(outControls); };
        inControls.field.onChanging = function () { updateFromField(inControls); };
        inControls.field.onChange = function () { updateFromField(inControls); };
        outControls.field.onChanging = function () { updateFromField(outControls); };
        outControls.field.onChange = function () { updateFromField(outControls); };
        inControls.enabled.onClick = function () { syncEnabledControls(inControls); updatePreview(); };
        outControls.enabled.onClick = function () { syncEnabledControls(outControls); updatePreview(); };
        sampleTextField.onChange = updatePreview;
        refreshButton.onClick = updatePreview;
        syncEnabledControls(inControls);
        syncEnabledControls(outControls);

        applyButton.onClick = function () {
            var inN = readNumber(inControls.field);
            var outN = readNumber(outControls.field);
            if (inControls.enabled.value && inN === null) {
                alert("Enter a valid IN preset number between 1 and " + AK_PRESETS.length + ".");
                return;
            }
            if (outControls.enabled.value && outN === null) {
                alert("Enter a valid OUT preset number between 1 and " + AK_PRESETS.length + ".");
                return;
            }
            if (!inControls.enabled.value && !outControls.enabled.value) {
                alert("Enable IN and/or OUT first.");
                return;
            }
            var comp = app.project.activeItem;
            var layer = null;
            if (comp && comp !== previewComp && comp.selectedLayers && comp.selectedLayers.length) {
                layer = comp.selectedLayers[0];
            }
            // Opening the preview makes it the active comp. Fall back to the user's editing context.
            if (!layer && launchComp && launchComp !== previewComp) {
                comp = launchComp;
                layer = comp.selectedLayers && comp.selectedLayers.length ? comp.selectedLayers[0] : launchLayer;
            }
            if (!comp || !layer || comp === previewComp) {
                alert("Select a native text layer in your work composition, then click Apply to Selected Text.");
                return;
            }
            if (inControls.enabled.value) applyPreset(parsePreset(AK_PRESETS[inN - 1]), "IN", inN, comp, layer);
            if (outControls.enabled.value) applyPreset(parsePreset(AK_PRESETS[outN - 1]), "OUT", outN, comp, layer);
        };

        closeButton.onClick = function () { win.close(); };
        win.onResizing = win.onResize = function () { this.layout.resize(); };
        updateDetails();
        win.layout.layout(true);
        // Build once on launch, then keep the same comp in sync with UI edits.
        updatePreview();
        return win;
    }

    try {
        var ui = buildUI(thisObj);
        if (ui && ui instanceof Window) ui.show();
    } catch (e) {
        alert("Animation Kit could not start.\n" + (e && e.message ? e.message : e));
        throw e;
    }
})(this);
